import {
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	backupPayloadHash,
	hashMutationPayload,
	NORMALIZED_SCHEMA_VERSION,
	resolveRuntimeDataDir,
	resolveStorageLayout,
	startDaemon,
} from "@codemem/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { openTestMemoryStore } from "../../core/src/test-utils.js";
import { ReadOnlyActor } from "../../core/src/writer-actor.js";
import { createMcpRpcClient, mcpRequestId } from "./rpc-client.js";

const projectConfigRace = vi.hoisted(() => ({
	path: "",
	replacement: "",
	growPath: "",
	growBy: "",
	descriptor: -1,
	initialSize: 0,
	bytesRead: 0,
}));

vi.mock("node:fs", async (importOriginal) => {
	const actual = await importOriginal<typeof import("node:fs")>();
	function replacePath(path: string) {
		const replacementPath = `${path}.replacement`;
		actual.writeFileSync(replacementPath, projectConfigRace.replacement, "utf8");
		actual.renameSync(replacementPath, path);
		projectConfigRace.path = "";
	}
	return {
		...actual,
		statSync(...args: Parameters<typeof actual.statSync>) {
			const info = actual.statSync(...args);
			if (String(args[0]) === projectConfigRace.path) {
				replacePath(String(args[0]));
			}
			return info;
		},
		openSync(...args: Parameters<typeof actual.openSync>) {
			const descriptor = actual.openSync(...args);
			const path = String(args[0]);
			if (path === projectConfigRace.path) replacePath(path);
			if (path === projectConfigRace.growPath) projectConfigRace.descriptor = descriptor;
			return descriptor;
		},
		fstatSync(...args: Parameters<typeof actual.fstatSync>) {
			const info = actual.fstatSync(...args);
			if (args[0] === projectConfigRace.descriptor && projectConfigRace.growPath) {
				projectConfigRace.initialSize = info.size;
				actual.appendFileSync(projectConfigRace.growPath, projectConfigRace.growBy, "utf8");
				projectConfigRace.growPath = "";
			}
			return info;
		},
		readSync(...args: Parameters<typeof actual.readSync>) {
			const bytesRead = actual.readSync(...args);
			if (args[0] === projectConfigRace.descriptor) projectConfigRace.bytesRead += bytesRead;
			return bytesRead;
		},
	};
});

afterEach(() => {
	projectConfigRace.path = "";
	projectConfigRace.replacement = "";
	projectConfigRace.growPath = "";
	projectConfigRace.growBy = "";
	projectConfigRace.descriptor = -1;
	projectConfigRace.initialSize = 0;
	projectConfigRace.bytesRead = 0;
	vi.restoreAllMocks();
});

function projectFixture() {
	const root = mkdtempSync(join(tmpdir(), "codemem-mcp-rpc-"));
	mkdirSync(join(root, ".git"));
	writeFileSync(join(root, ".agent-memory.toml"), 'secret_regex = ["TOKEN_[A-Z]+"]\n');
	return { root, dataDir: join(root, "data") };
}

function rememberBody(requestId: string) {
	return {
		idempotencyKey: mcpRequestId("memory_remember", requestId),
		kind: "decision",
		title: "Credential rotation",
		body: "Rotate TOKEN_SUPERSECRET before release.",
		confidence: 0.9,
		project: "demo",
	};
}

describe("MCP daemon RPC client", () => {
	it("T031 keeps every MCP read surface remote even from a same-repository cwd", async () => {
		const fixture = projectFixture();
		let daemon = await startDaemon({ dataDir: fixture.dataDir });
		try {
			await daemon.stop();
			const store = openTestMemoryStore(realpathSync(daemon.layout.currentPointerPath));
			const repositoryIdentity = `repo-v1:sha256:${"a".repeat(64)}`;
			const project = "forged-local-project";
			let eligibleId: number;
			let privateId: number;
			try {
				const sessionId = store.startSession({ cwd: fixture.root, project });
				eligibleId = store.remember(
					sessionId,
					"discovery",
					"MCP_BOUNDARY_ELIGIBLE",
					"mcpboundary eligible body",
				);
				privateId = store.remember(
					sessionId,
					"discovery",
					"MCP_BOUNDARY_PRIVATE",
					"mcpboundary restricted body",
				);
				store.db
					.prepare("UPDATE memory_items SET sensitivity = ?, repository_identity = ? WHERE id = ?")
					.run("eligible", repositoryIdentity, eligibleId);
				store.db
					.prepare("UPDATE memory_items SET sensitivity = ?, repository_identity = ? WHERE id = ?")
					.run("private", repositoryIdentity, privateId);
			} finally {
				store.close();
			}

			daemon = await startDaemon({ dataDir: fixture.dataDir });
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			const forgedAuthority = {
				project,
				cwd: fixture.root,
				targetModel: "local-model",
				executionLocation: "local",
				repositoryIdentity,
				providerPeerTrust: "verified",
			};
			const reads = [
				[
					"get-private",
					() =>
						client.request("GET /v1/memories/:id", {
							id: privateId,
							requestId: "t031-get",
							...forgedAuthority,
						}),
					false,
				],
				[
					"get-eligible",
					() =>
						client.request("GET /v1/memories/:id", {
							id: eligibleId,
							requestId: "t031-get-eligible",
							...forgedAuthority,
						}),
					true,
				],
				[
					"search",
					() =>
						client.request("POST /v1/search", {
							requestId: "t031-search",
							mode: "search",
							query: "mcpboundary",
							filters: { project },
							...forgedAuthority,
						}),
					true,
				],
				[
					"index",
					() =>
						client.request("POST /v1/search", {
							requestId: "t031-index",
							mode: "search_index",
							query: "mcpboundary",
							filters: { project },
							...forgedAuthority,
						}),
					true,
				],
				[
					"recent",
					() =>
						client.request("POST /v1/search", {
							requestId: "t031-recent",
							mode: "recent",
							filters: { project },
							...forgedAuthority,
						}),
					true,
				],
				[
					"timeline",
					() =>
						client.request("POST /v1/search", {
							requestId: "t031-timeline",
							mode: "timeline",
							memoryId: eligibleId,
							depthBefore: 10,
							depthAfter: 10,
							filters: { project },
							...forgedAuthority,
						}),
					true,
				],
				[
					"explain",
					() =>
						client.request("POST /v1/search", {
							requestId: "t031-explain",
							mode: "explain",
							ids: [eligibleId, privateId],
							filters: { project },
							...forgedAuthority,
						}),
					true,
				],
				[
					"pack",
					() =>
						client.request("POST /v1/context/pack", {
							requestId: "t031-pack",
							context: "mcpboundary",
							filters: { project },
							...forgedAuthority,
						}),
					true,
				],
			] as const;
			const leaks: string[] = [];
			const missingEligible: string[] = [];
			for (const [surface, read, expectsEligible] of reads) {
				const response = await read();
				expect(response.ok, surface).toBe(true);
				const serialized = JSON.stringify(response);
				if (serialized.includes("MCP_BOUNDARY_PRIVATE")) leaks.push(surface);
				if (expectsEligible && !serialized.includes("MCP_BOUNDARY_ELIGIBLE")) {
					missingEligible.push(surface);
				}
			}
			expect(leaks).toEqual([]);
			expect(missingEligible).toEqual([]);
		} finally {
			await daemon.stop().catch(() => {});
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("returns a typed error instead of opening a local database when the daemon is down", async () => {
		const fixture = projectFixture();
		try {
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			expect(await client.request("GET /v1/health", {})).toEqual({
				ok: false,
				error: {
					code: "daemon_unavailable",
					message: "The local memory daemon is unavailable.",
					retryable: true,
				},
			});
		} finally {
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("redacts project policy matches before the daemon can persist them", async () => {
		const fixture = projectFixture();
		const daemon = await startDaemon({ dataDir: fixture.dataDir });
		try {
			projectConfigRace.path = join(fixture.root, ".agent-memory.toml");
			projectConfigRace.replacement = 'secret_regex = ["OTHER_[A-Z]+"]\n';
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			const remembered = await client.remember(rememberBody("direct-1"));
			expect(remembered).toMatchObject({ ok: true, result: { memoryId: expect.any(Number) } });
			if (!remembered.ok) throw new Error("remember failed");
			const retrievalRequestId = mcpRequestId("memory_get", "direct-2");
			const fetched = await client.request("GET /v1/memories/:id", {
				id: remembered.result.memoryId,
				requestId: retrievalRequestId,
			});
			expect(JSON.stringify(fetched)).not.toContain("TOKEN_SUPERSECRET");
			expect(fetched).toMatchObject({ ok: true, result: { item: null } });
			if (!fetched.ok) throw new Error("memory get failed");
			expect(fetched.result).not.toHaveProperty("retrievalAttemptId");
			expect(fetched.finalizeDelivery).toBeUndefined();
			const deliveryReader = ReadOnlyActor.open(realpathSync(daemon.layout.currentPointerPath));
			try {
				expect(
					deliveryReader
						.prepare("SELECT delivery_status FROM retrieval_attempts WHERE request_id = ?")
						.pluck()
						.get(retrievalRequestId),
				).toBe("not_attempted");
			} finally {
				deliveryReader.close();
			}
			const configPath = join(fixture.root, ".agent-memory.toml");
			expect(readFileSync(configPath, "utf8")).toContain("OTHER_[A-Z]+");

			writeFileSync(configPath, 'secret_regex = ["TOKEN_[A-Z]+"]\n');
			projectConfigRace.growPath = configPath;
			projectConfigRace.growBy = `#${"x".repeat(70_000)}\n`;
			expect(await client.remember(rememberBody("growing-config"))).toMatchObject({ ok: true });
			expect(projectConfigRace.bytesRead).toBe(projectConfigRace.initialSize);
			writeFileSync(configPath, 'secret_regex = ["TOKEN_[A-Z]+"]\n');

			const idempotencyKey = "mcp-event-redaction";
			const event = {
				schemaVersion: NORMALIZED_SCHEMA_VERSION,
				eventId: "mcp-event-redaction-1",
				idempotencyKey,
				agent: "opencode",
				nativeSessionId: "mcp-redaction-session",
				projectKey: "demo",
				workspaceKey: fixture.root,
				cwd: fixture.root,
				kind: "user_prompted",
				occurredAt: "2026-08-14T00:00:00.000Z",
				payload: {
					text: "TOKEN_SUPERSECRET <private>hidden</private> <local-only>device</local-only>",
				},
				sourceHash: hashMutationPayload({ secret: "TOKEN_SUPERSECRET" }),
				sensitivity: "normal",
			};
			expect(
				await client.requestWithSpool("POST /v1/events", { idempotencyKey, event }),
			).toMatchObject({ ok: true, result: { receiptId: expect.any(String) } });

			const reader = ReadOnlyActor.open(realpathSync(daemon.layout.currentPointerPath));
			try {
				const row = reader
					.prepare("SELECT payload_json FROM raw_events WHERE event_id = ?")
					.get("mcp-event-redaction-1") as { payload_json: string };
				expect(row.payload_json).not.toContain("TOKEN_SUPERSECRET");
				expect(row.payload_json).not.toContain("hidden");
				expect(row.payload_json).not.toContain("device");
				expect(JSON.parse(row.payload_json)).toMatchObject({
					_normalized: {
						sensitivity: "secret",
						private_content_omitted: true,
						local_only: true,
					},
				});
			} finally {
				reader.close();
			}

			writeFileSync(join(fixture.root, ".agent-memory.toml"), 'secret_regex = ["(a+)+$"]\n');
			const degradedIdempotencyKey = "mcp-event-degraded";
			const degradedProject = "MCP_EVENT_DEGRADED_PROJECT";
			const degradedEvent = {
				...event,
				eventId: "mcp-event-degraded-1",
				idempotencyKey: degradedIdempotencyKey,
				projectKey: degradedProject,
				payload: { text: `${"a".repeat(26)}!` },
				sourceHash: hashMutationPayload({ degraded: true }),
			};
			expect(
				await client.requestWithSpool("POST /v1/events", {
					idempotencyKey: degradedIdempotencyKey,
					event: degradedEvent,
				}),
			).toMatchObject({ ok: true, result: { receiptId: expect.any(String) } });

			const degradedReader = ReadOnlyActor.open(realpathSync(daemon.layout.currentPointerPath));
			try {
				const row = degradedReader
					.prepare(
						"SELECT payload_json, sensitivity, capture_state, safe_error_code FROM raw_events WHERE event_id = ?",
					)
					.get("mcp-event-degraded-1") as {
					payload_json: string;
					sensitivity: string;
					capture_state: string;
					safe_error_code: string;
				};
				expect(row.payload_json).not.toContain(degradedProject);
				expect(JSON.parse(row.payload_json)).toEqual({});
				expect(row).toMatchObject({
					sensitivity: "secret",
					capture_state: "quarantined",
					safe_error_code: "redaction_degraded",
				});
			} finally {
				degradedReader.close();
			}
		} finally {
			await daemon.stop();
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("persists degraded remember diagnostics across daemon restart", async () => {
		const fixture = projectFixture();
		writeFileSync(join(fixture.root, ".agent-memory.toml"), 'secret_regex = ["(a+)+$"]\n');
		let daemon = await startDaemon({ dataDir: fixture.dataDir });
		try {
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			const secret = `${"a".repeat(26)}!`;
			const remembered = await client.remember({
				...rememberBody("degraded-direct"),
				title: secret,
				body: "MCP_DEGRADED_PRIVATE",
				project: "MCP_DEGRADED_PROJECT",
			});
			expect(remembered).toMatchObject({ ok: true, result: { memoryId: expect.any(Number) } });
			if (!remembered.ok) throw new Error("degraded remember failed");

			const reader = ReadOnlyActor.open(realpathSync(daemon.layout.currentPointerPath));
			try {
				const row = reader
					.prepare(
						`SELECT m.title, m.body_text, m.metadata_json, s.project
						 FROM memory_items m JOIN sessions s ON s.id = m.session_id WHERE m.id = ?`,
					)
					.get(remembered.result.memoryId) as {
					title: string;
					body_text: string;
					metadata_json: string;
					project: string | null;
				};
				expect(row.title).toBe("");
				expect(row.body_text).toBe("");
				expect(JSON.parse(row.metadata_json)).toMatchObject({ redaction_degraded: true });
				expect(JSON.stringify(row)).not.toContain(secret);
				expect(JSON.stringify(row)).not.toContain("MCP_DEGRADED_PRIVATE");
				expect(JSON.stringify(row)).not.toContain("MCP_DEGRADED_PROJECT");
			} finally {
				reader.close();
			}

			expect(await client.request("GET /v1/doctor", {})).toMatchObject({
				ok: true,
				result: { diagnostics: { redaction: { status: "warning", degradedDeliveries: 1 } } },
			});
			await daemon.stop();
			daemon = await startDaemon({ dataDir: fixture.dataDir });
			expect(await client.request("GET /v1/doctor", {})).toMatchObject({
				ok: true,
				result: { diagnostics: { redaction: { status: "warning", degradedDeliveries: 1 } } },
			});
		} finally {
			await daemon.stop().catch(() => {});
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("P1-T042-03-mcp-remember-spool", async () => {
		const fixture = projectFixture();
		const originalDataDir = process.env.CODEMEM_DATA_DIR;
		const originalDb = process.env.CODEMEM_DB;
		try {
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			const scopedId = mcpRequestId("memory_remember", "spool-1", "mcp-session-a");
			expect(scopedId).toBe(mcpRequestId("memory_remember", "spool-1", "mcp-session-a"));
			expect(scopedId).not.toBe(mcpRequestId("memory_remember", "spool-1", "mcp-session-b"));
			const body = { ...rememberBody("spool-1"), idempotencyKey: scopedId };
			const queued = await client.remember(body);
			const duplicate = await client.remember(body);
			expect(queued).toMatchObject({ ok: true, result: { status: "queued", duplicate: false } });
			expect(duplicate).toMatchObject({ ok: true, result: { status: "queued", duplicate: true } });

			const readyDir = join(fixture.dataDir, "control", "spool", "ready");
			const files = readdirSync(readyDir);
			expect(files).toHaveLength(1);
			const serialized = readFileSync(join(readyDir, files[0]), "utf8");
			expect(serialized).not.toContain("TOKEN_SUPERSECRET");
			expect(JSON.parse(serialized)).toMatchObject({
				method: "POST /v1/memories/record",
				idempotencyKey: body.idempotencyKey,
				redaction: { sensitivity: "secret" },
			});

			delete process.env.CODEMEM_DATA_DIR;
			const customDbPath = join(fixture.root, "mcp.sqlite");
			process.env.CODEMEM_DB = customDbPath;
			const customDataDir = resolveRuntimeDataDir({ dbPath: customDbPath });
			const daemon = await startDaemon({ dataDir: customDataDir });
			try {
				const envClient = createMcpRpcClient({ cwd: () => fixture.root });
				expect(await envClient.request("GET /v1/health", {})).toMatchObject({ ok: true });
				await daemon.stop();
				const envBody = rememberBody("spool-env-db");
				expect(await envClient.remember(envBody)).toMatchObject({
					ok: true,
					result: { status: "queued", duplicate: false },
				});
				const envReadyDir = join(customDataDir, "control", "spool", "ready");
				expect(readdirSync(envReadyDir)).toHaveLength(1);
			} finally {
				await daemon.stop().catch(() => {});
				rmSync(customDataDir, { recursive: true, force: true });
			}
		} finally {
			if (originalDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
			else process.env.CODEMEM_DATA_DIR = originalDataDir;
			if (originalDb === undefined) delete process.env.CODEMEM_DB;
			else process.env.CODEMEM_DB = originalDb;
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("routes backup create, list, and verify through the daemon", async () => {
		const fixture = projectFixture();
		const daemon = await startDaemon({ dataDir: fixture.dataDir });
		try {
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			const reason = "manual TOKEN_SUPERSECRET backup";
			expect(
				await client.request("POST /v1/backup/create", {
					operationId: "mcp-backup",
					reason,
					payloadHash: backupPayloadHash(reason),
				}),
			).toMatchObject({
				ok: true,
				result: { backupId: "mcp-backup", manifestHash: expect.any(String) },
			});
			const sidecar = readFileSync(
				join(resolveStorageLayout(fixture.dataDir).backupsDir, "mcp-backup.json"),
				"utf8",
			);
			expect(sidecar).not.toContain("TOKEN_SUPERSECRET");
			expect(sidecar).toContain("[REDACTED:user_1]");
			expect(await client.request("GET /v1/backup/list", {})).toMatchObject({
				ok: true,
				result: { backups: [expect.objectContaining({ backupId: "mcp-backup", valid: true })] },
			});
			expect(
				await client.request("POST /v1/backup/verify", { backupId: "mcp-backup" }),
			).toMatchObject({ ok: true, result: { backupId: "mcp-backup", valid: true } });
			writeFileSync(join(fixture.root, ".agent-memory.toml"), 'secret_regex = ["["]\n');
			const degradedReason = `${"a".repeat(26)}!`;
			const originalBackup = ReadOnlyActor.prototype.backup;
			const delayedBackup = vi
				.spyOn(ReadOnlyActor.prototype, "backup")
				.mockImplementationOnce(async function (destinationFile, options) {
					await new Promise((resolve) => setTimeout(resolve, 2_100));
					return originalBackup.call(this, destinationFile, options);
				});
			const startedAt = Date.now();
			expect(
				await client.request("POST /v1/backup/create", {
					operationId: "mcp-backup-degraded",
					reason: degradedReason,
					payloadHash: backupPayloadHash(degradedReason),
				}),
			).toMatchObject({ ok: true, result: { backupId: "mcp-backup-degraded" } });
			expect(Date.now() - startedAt).toBeGreaterThanOrEqual(2_000);
			expect(delayedBackup).toHaveBeenCalledTimes(1);
			delayedBackup.mockRestore();
			const degradedSidecar = readFileSync(
				join(resolveStorageLayout(fixture.dataDir).backupsDir, "mcp-backup-degraded.json"),
				"utf8",
			);
			expect(degradedSidecar).not.toContain(degradedReason);
			expect(degradedSidecar).toContain("[REDACTED:degraded]");
		} finally {
			await daemon.stop();
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("P1-T046-02-maintenance-spool", async () => {
		const fixture = projectFixture();
		const layout = resolveStorageLayout(fixture.dataDir);
		mkdirSync(layout.controlDir, { recursive: true, mode: 0o700 });
		const server = createServer((socket) => {
			socket.once("data", () => {
				socket.end(
					`${JSON.stringify({
						error: {
							code: "maintenance_mode",
							message: "The daemon is in maintenance mode.",
							retryable: true,
						},
					})}\n`,
				);
			});
		});
		await new Promise<void>((resolve, reject) => {
			server.once("error", reject);
			server.listen(layout.socketPath, resolve);
		});
		try {
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			const body = rememberBody("maintenance-spool");
			expect(await client.remember(body)).toMatchObject({
				ok: true,
				result: { status: "queued", duplicate: false },
			});
			const readyDir = join(layout.spoolDir, "ready");
			const files = readdirSync(readyDir);
			expect(files).toHaveLength(1);
			expect(readFileSync(join(readyDir, files[0]), "utf8")).not.toContain("TOKEN_SUPERSECRET");
		} finally {
			await new Promise<void>((resolve) => server.close(() => resolve()));
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("T018 preserves a retryable capture-saturation receipt and spools the retry", async () => {
		const fixture = projectFixture();
		const layout = resolveStorageLayout(fixture.dataDir);
		mkdirSync(layout.controlDir, { recursive: true, mode: 0o700 });
		const server = createServer((socket) => {
			socket.once("data", () => {
				socket.end(
					`${JSON.stringify({
						error: {
							code: "capture_saturated",
							message: "Capture admission is saturated; retry through the spool.",
							retryable: true,
						},
					})}\n`,
				);
			});
		});
		await new Promise<void>((resolve, reject) => {
			server.once("error", reject);
			server.listen(layout.socketPath, resolve);
		});
		const idempotencyKey = ["t018", "mcp", "saturation"].join("-");
		const event = {
			schemaVersion: NORMALIZED_SCHEMA_VERSION,
			eventId: "t018-mcp-saturation",
			idempotencyKey,
			agent: "codex",
			nativeSessionId: "t018-mcp-session",
			projectKey: "t018-mcp-project",
			workspaceKey: fixture.root,
			cwd: fixture.root,
			kind: "user_prompted",
			occurredAt: "2026-08-31T00:00:00.000Z",
			payload: { text: "retryable saturation is spoolable" },
			sourceHash: hashMutationPayload({ idempotencyKey }),
			sensitivity: "normal",
		};
		try {
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			expect(await client.request("POST /v1/events", { idempotencyKey, event })).toEqual({
				ok: false,
				error: {
					code: "capture_saturated",
					message: "Capture admission is saturated; retry through the spool.",
					retryable: true,
				},
			});
			expect(
				await client.requestWithSpool("POST /v1/events", { idempotencyKey, event }),
			).toMatchObject({
				ok: true,
				result: { status: "queued", duplicate: false },
			});
			expect(readdirSync(join(layout.spoolDir, "ready"))).toHaveLength(1);
		} finally {
			await new Promise<void>((resolve) => server.close(() => resolve()));
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});

	it("preserves doctor control fields when project policy is unavailable", async () => {
		const fixture = projectFixture();
		writeFileSync(join(fixture.root, ".agent-memory.toml"), "x".repeat(65 * 1024));
		const layout = resolveStorageLayout(fixture.dataDir);
		mkdirSync(layout.controlDir, { recursive: true, mode: 0o700 });
		const receivedBodies: Record<string, unknown>[] = [];
		const server = createServer((socket) => {
			socket.once("data", (data) => {
				const request = JSON.parse(String(data).trim()) as {
					id: string;
					body: Record<string, unknown>;
				};
				receivedBodies.push(request.body);
				socket.end(`${JSON.stringify({ id: request.id, result: { status: "accepted" } })}\n`);
			});
		});
		await new Promise<void>((resolve, reject) => {
			server.once("error", reject);
			server.listen(layout.socketPath, resolve);
		});
		const expectedBody = {
			id: 7,
			producerReceiptId: "doctor-receipt-7",
			expectedRole: "summary",
			expectedProviderFingerprint: `sha256:${"a".repeat(64)}`,
			expectedManifestFingerprint: `sha256:${"b".repeat(64)}`,
			expectedAttemptCount: 3,
			expectedClaimGeneration: 2,
		};
		try {
			const client = createMcpRpcClient({ dataDir: fixture.dataDir, cwd: () => fixture.root });
			expect(
				await client.request("GET /v1/processing-jobs/:id", { id: 7, ignored: "field" }),
			).toMatchObject({ ok: true, result: { status: "accepted" } });
			expect(
				await client.request("POST /v1/processing-jobs/:id/doctor-retry", expectedBody),
			).toMatchObject({ ok: true, result: { status: "accepted" } });
			expect(receivedBodies).toEqual([{ id: 7 }, expectedBody]);
		} finally {
			await new Promise<void>((resolve) => server.close(() => resolve()));
			rmSync(fixture.root, { recursive: true, force: true });
		}
	});
});
