import { globSync, readFileSync } from "node:fs";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { describe, expect, it } from "vitest";
import { createCodememMcpServer } from "./index.js";
import type { McpRpcClient } from "./rpc-client.js";

const ALLOWED_TOOLS = [
	"memory_expand",
	"memory_explain",
	"memory_get",
	"memory_get_observations",
	"memory_pack",
	"memory_recent",
	"memory_remember",
	"memory_schema",
	"memory_search",
	"memory_search_index",
	"memory_status",
	"memory_timeline",
];

async function connect(clientImpl?: McpRpcClient) {
	const server = createCodememMcpServer({
		client: clientImpl,
		defaultProject: "demo",
		envProject: null,
	});
	const client = new Client({ name: "codemem-test", version: "1" });
	const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
	await server.connect(serverTransport);
	await client.connect(clientTransport);
	return {
		client,
		close: async () => {
			await client.close();
			await server.close();
		},
	};
}

describe("Phase 1 MCP stdio RPC surface", () => {
	it("exports a side-effect-free factory from the package root", () => {
		expect(createCodememMcpServer).toBeTypeOf("function");
	});

	it("P1-T042-01-mcp-minimal-tools", async () => {
		const connection = await connect();
		try {
			const listed = await connection.client.listTools();
			expect(listed.tools.map((tool) => tool.name).toSorted()).toEqual(ALLOWED_TOOLS);
			// The zero-argument tools declare `inputSchema: {}` rather than omitting it:
			// omitting it makes the SDK advertise its own empty-object constant, which
			// drops the $schema keyword these two tools have always published.
			for (const name of ["memory_status", "memory_schema"]) {
				const tool = listed.tools.find((candidate) => candidate.name === name);
				expect(tool?.inputSchema).toEqual({
					type: "object",
					properties: {},
					$schema: "http://json-schema.org/draft-07/schema#",
				});
			}
			// memory_expand publishes its id pattern on the wire: zod copies
			// RegExp.source into the JSON Schema, so `[0-9]` vs `\d` is an
			// observable difference for clients, not a style choice.
			const expand = listed.tools.find((candidate) => candidate.name === "memory_expand");
			expect(
				(expand?.inputSchema as { properties?: { ids?: { items?: unknown } } }).properties?.ids
					?.items,
			).toMatchObject({
				anyOf: [
					{ type: "integer", exclusiveMinimum: 0 },
					{ type: "string", pattern: "^[1-9][0-9]*$" },
				],
			});
		} finally {
			await connection.close();
		}
	});

	it("P1-T042-02-mcp-user-mutation-denied", async () => {
		const connection = await connect();
		try {
			for (const name of [
				"memory_forget",
				"memory_confirm",
				"memory_pin",
				"memory_unpin",
				"memory_retract",
				"memory_mark_wrong",
				"memory_bulk_delete",
			]) {
				const result = await connection.client.callTool({ name, arguments: {} });
				expect(result).toMatchObject({ isError: true });
				expect(JSON.stringify(result)).toContain(`Tool ${name} not found`);
			}
		} finally {
			await connection.close();
		}
	});

	it("maps every read tool to its fixed daemon endpoint and mode", async () => {
		const calls: Array<{ method: string; body: Record<string, unknown> }> = [];
		const deliveries: string[] = [];
		const remembers: Record<string, unknown>[] = [];
		const retrieved = (result: Record<string, unknown>) => ({
			ok: true as const,
			result,
			finalizeDelivery: async (status: "handed_off" | "failed") => {
				deliveries.push(status);
			},
		});
		const fake: McpRpcClient = {
			async request(method, body) {
				calls.push({ method, body });
				if (method === "GET /v1/health") return { ok: true, result: { status: "ok" } };
				if (method === "GET /v1/memories/:id") return retrieved({ item: { id: 7 } });
				if (method === "POST /v1/context/pack") return retrieved({ pack: {} });
				if (body.mode === "search") {
					return retrieved({
						items: [
							{
								id: 7,
								title: "Result",
								kind: "decision",
								body_text: "Public body",
								confidence: 0.9,
								score: 0.8,
								session_id: 3,
								metadata: { source: "test" },
								created_at: "internal-only",
							},
						],
					});
				}
				if (body.mode === "explain") return retrieved({ items: { items: [] } });
				return retrieved({ items: [], status: "ok" });
			},
			async remember(body) {
				remembers.push(body);
				return { ok: true, result: { memoryId: 1 } };
			},
		};
		const connection = await connect(fake);
		try {
			await connection.client.callTool({
				name: "memory_remember",
				arguments: { kind: "decision", title: "title", body: "body" },
			});
			await connection.client.callTool({ name: "memory_status", arguments: {} });
			await connection.client.callTool({ name: "memory_get", arguments: { memory_id: 7 } });
			await connection.client.callTool({
				name: "memory_get_observations",
				arguments: { ids: [7] },
			});
			const search = await connection.client.callTool({
				name: "memory_search",
				arguments: { query: "query" },
			});
			await connection.client.callTool({
				name: "memory_search_index",
				arguments: { query: "query" },
			});
			await connection.client.callTool({ name: "memory_recent", arguments: {} });
			await connection.client.callTool({ name: "memory_explain", arguments: { ids: [7] } });
			await connection.client.callTool({
				name: "memory_timeline",
				arguments: { memory_id: 7 },
			});
			await connection.client.callTool({ name: "memory_expand", arguments: { ids: [7] } });
			await connection.client.callTool({ name: "memory_pack", arguments: { context: "query" } });
			expect(calls.map(({ method, body }) => [method, body.mode ?? null])).toEqual([
				["GET /v1/health", null],
				["GET /v1/memories/:id", null],
				["POST /v1/search", "get_many"],
				["POST /v1/search", "search"],
				["POST /v1/search", "search_index"],
				["POST /v1/search", "recent"],
				["POST /v1/search", "explain"],
				["POST /v1/search", "timeline"],
				["POST /v1/search", "expand"],
				["POST /v1/context/pack", null],
			]);
			expect(JSON.parse((search.content[0] as { text: string }).text)).toEqual({
				items: [
					{
						id: 7,
						title: "Result",
						kind: "decision",
						body: "Public body",
						confidence: 0.9,
						score: 0.8,
						session_id: 3,
						metadata: { source: "test" },
					},
				],
			});
			expect(remembers).toEqual([
				expect.objectContaining({
					idempotencyKey: expect.stringMatching(/^[a-f0-9]{64}$/),
					kind: "decision",
					title: "title",
					body: "body",
					confidence: 0.5,
				}),
			]);
			expect(calls.every(({ body }) => !Object.hasOwn(body, "store"))).toBe(true);
			expect(deliveries).toEqual(Array(9).fill("handed_off"));
			const transformFailure = {
				get id(): never {
					throw new Error("injected MCP response transform failure");
				},
			};
			const originalRequest = fake.request;
			fake.request = async (method, body) =>
				body.mode === "search"
					? retrieved({ items: [transformFailure] })
					: originalRequest(method, body);
			expect(
				await connection.client.callTool({
					name: "memory_search",
					arguments: { query: "transform failure" },
				}),
			).toMatchObject({ isError: true });
			expect(deliveries.at(-1)).toBe("failed");

			const secondSession = await connect(fake);
			try {
				await secondSession.client.callTool({
					name: "memory_remember",
					arguments: { kind: "decision", title: "title", body: "body" },
				});
			} finally {
				await secondSession.close();
			}
			expect(remembers).toHaveLength(2);
			expect(remembers[1]?.idempotencyKey).not.toBe(remembers[0]?.idempotencyKey);
		} finally {
			await connection.close();
		}
	});

	it("T031 treats project and local-looking labels only as untrusted read narrowing", async () => {
		const calls: Array<{ method: string; body: Record<string, unknown> }> = [];
		const sentinel = "MCP_DAEMON_RESULT_SENTINEL";
		const fake: McpRpcClient = {
			async request(method, body) {
				calls.push({ method, body });
				const item = { id: 7, title: sentinel, body_text: "restricted daemon body" };
				if (method === "GET /v1/memories/:id") return { ok: true, result: { item } };
				if (method === "POST /v1/context/pack") {
					return { ok: true, result: { pack: { title: sentinel, item_ids: [7] } } };
				}
				if (body.mode === "explain") {
					return { ok: true, result: { items: { items: [item], errors: [] } } };
				}
				return { ok: true, result: { items: [item] } };
			},
			async requestWithSpool() {
				throw new Error("unexpected mutation");
			},
			async remember() {
				throw new Error("unexpected mutation");
			},
		};
		const connection = await connect(fake);
		const forged = {
			project: "forged-project",
			cwd: "/tmp/looks-local",
			model: "local-model",
			caller: "trusted-local-caller",
			execution_location: "local",
			repository_identity: `repo-v1:sha256:${"a".repeat(64)}`,
			provider_peer_trust: "verified",
		};
		try {
			const reads = [
				["memory_get", { memory_id: 7, ...forged }],
				["memory_search", { query: "query", ...forged }],
				["memory_search_index", { query: "query", ...forged }],
				["memory_recent", forged],
				["memory_timeline", { memory_id: 7, ...forged }],
				["memory_explain", { ids: [7], ...forged }],
				["memory_pack", { context: "query", ...forged }],
			] as const;
			for (const [name, arguments_] of reads) {
				const result = await connection.client.callTool({ name, arguments: arguments_ });
				expect(JSON.stringify(result), name).toContain(sentinel);
			}
			expect(calls).toHaveLength(reads.length);
			for (const { body } of calls) {
				const serialized = JSON.stringify(body);
				expect(serialized).not.toMatch(
					/looks-local|local-model|trusted-local-caller|repository_identity|execution_location|provider_peer_trust/,
				);
				const narrowedProject =
					body.project ?? (body.filters as Record<string, unknown> | undefined)?.project;
				expect(narrowedProject).toBe("forged-project");
			}
		} finally {
			await connection.close();
		}
	});

	it("P1-T042-04-mcp-no-db-fallback", () => {
		const files = globSync("**/*.ts", { cwd: import.meta.dirname }).filter(
			(path) => !path.endsWith(".test.ts"),
		);
		const source = files
			.map((path) => readFileSync(`${import.meta.dirname}/${path}`, "utf8"))
			.join("\n");
		expect(source).not.toMatch(/\bMemoryStore\b|\bresolveDbPath\b|better-sqlite3|\bstore\.db\b/);
	});
});
