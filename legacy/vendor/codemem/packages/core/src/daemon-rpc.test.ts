import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createConnection, createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { attachDaemonRpc } from "./daemon-rpc.js";
import { connect } from "./db.js";
import { compileProviderDestinationBoundary } from "./destination-boundary.js";
import * as core from "./index.js";
import { resolveRepositoryIdentity } from "./project.js";
import {
	acquireCapabilityLifecycleLock,
	activateCapabilityManifest,
	writeCapabilityManifestGeneration,
} from "./storage.js";
import { openTestMemoryStore } from "./test-utils.js";
import { ReadOnlyActor } from "./writer-actor.js";

const created: Array<{ stop: () => Promise<void> }> = [];
const dirs: string[] = [];

function tempDataDir(): string {
	const dir = mkdtempSync(join(tmpdir(), "codemem-rpc-"));
	dirs.push(dir);
	return join(dir, "data");
}

function handshake(overrides: Partial<core.RpcRequest> = {}): core.RpcRequest {
	return {
		id: "req-1",
		method: "GET /v1/health",
		adapter_version: "1",
		native_cli_version: "1",
		normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
		local_api_version: core.LOCAL_API_VERSION,
		capability_hash: core.RPC_CAPABILITY_HASH,
		...overrides,
	};
}

function capabilityManifest(
	endpointUrl: string,
	modelId = "t008-summary-model",
	baseConfigurationFingerprint?: string,
) {
	return core.compileDefaultCapabilityManifest(
		{
			version: 1,
			role: "summary",
			state: "enabled",
			wireProtocol: "openai_chat_completions_v1",
			modelId,
			modelRevision: "1",
			endpointUrl,
			credentialRef: { kind: "none" },
		},
		[],
		baseConfigurationFingerprint,
	);
}

function activateManifest(dataDir: string, manifest: core.EffectiveCapabilityManifestV1): void {
	const layout = core.resolveStorageLayout(dataDir);
	writeCapabilityManifestGeneration(layout, manifest);
	const lease = acquireCapabilityLifecycleLock(layout);
	try {
		activateCapabilityManifest(layout, manifest.configurationFingerprint, lease);
	} finally {
		lease.close();
	}
}

function rpcResult(response: core.RpcSuccess | core.TypedRpcError): Record<string, unknown> {
	if ("error" in response) throw new Error(`RPC failed: ${response.error.code}`);
	return response.result;
}

function doctorCapability(response: core.RpcSuccess | core.TypedRpcError): Record<string, unknown> {
	const diagnostics = Reflect.get(rpcResult(response), "diagnostics");
	if (!diagnostics || typeof diagnostics !== "object")
		throw new Error("doctor diagnostics missing");
	const capability =
		Reflect.get(diagnostics, "capability") ?? Reflect.get(diagnostics, "runtimeCapability");
	expect(capability, "doctor must expose the frozen capability state").toBeDefined();
	return capability as Record<string, unknown>;
}

function featureEnabled(runtime: Record<string, unknown>, feature: string): boolean | undefined {
	const direct = Reflect.get(runtime, `${feature}Enabled`);
	if (typeof direct === "boolean") return direct;
	const nested = Reflect.get(runtime, feature);
	if (typeof nested === "boolean") return nested;
	if (!nested || typeof nested !== "object") return undefined;
	const enabled = Reflect.get(nested, "enabled");
	if (typeof enabled === "boolean") return enabled;
	const state = Reflect.get(nested, "state");
	if (typeof state === "string") return state === "enabled" || state === "ready";
	return undefined;
}

function normalizedEvent(id: string): Record<string, unknown> {
	return {
		schemaVersion: core.NORMALIZED_SCHEMA_VERSION,
		eventId: id,
		idempotencyKey: id,
		agent: "codex",
		nativeSessionId: "t008-session",
		projectKey: "t008-project",
		workspaceKey: "t008-workspace",
		cwd: process.cwd(),
		kind: "user_prompted",
		occurredAt: new Date(0).toISOString(),
		payload: { text: "t008 safe capture" },
		sourceHash: "a".repeat(64),
		sensitivity: "normal",
	};
}

async function waitForDaemonJob(
	socketPath: string,
	jobId: string,
): Promise<Record<string, unknown> | null> {
	for (let attempt = 0; attempt < 100; attempt++) {
		const response = await core.callDaemonRpc(
			socketPath,
			handshake({
				id: `job-${jobId}-${attempt}`,
				method: "GET /v1/jobs/:id",
				body: { id: jobId },
			}),
		);
		if ("result" in response) {
			const job = (response.result.job as Record<string, unknown> | null) ?? null;
			if (job?.state === "completed" || job?.state === "failed") return job;
		}
		await new Promise((resolve) => setTimeout(resolve, 10));
	}
	return null;
}

afterEach(async () => {
	for (const handle of created.splice(0)) {
		try {
			await handle.stop();
		} catch {
			// cleanup
		}
	}
	for (const dir of dirs.splice(0)) {
		rmSync(dir, { recursive: true, force: true });
	}
});

describe("Phase 1 daemon RPC", () => {
	it("P1-T035-01-handshake-version", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const response = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ local_api_version: core.LOCAL_API_VERSION + 1 }),
		);
		expect(response).toMatchObject({
			error: { code: "protocol_mismatch", retryable: false },
		});
		const schema = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ normalized_schema_version: 0 }),
		);
		expect(schema).toMatchObject({ error: { code: "protocol_mismatch" } });
	});

	it("P1-T035-02-schema-allowlist", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const unknownMethod = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "POST /v1/not-a-method" }),
		);
		expect(unknownMethod).toMatchObject({ error: { code: "unknown_method" } });
		const unknownField = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ extra: true } as unknown as core.RpcRequest),
		);
		expect(unknownField).toMatchObject({ error: { code: "unknown_field" } });
		const unknownBodyField = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "GET /v1/health", body: { surprise: 1 } }),
		);
		expect(unknownBodyField).toMatchObject({ error: { code: "unknown_field" } });
	});

	it("P1-T035-03-size-and-deadline", async () => {
		expect(core.rpcDeadlineForMethod("GET /v1/health")).toBe(core.RPC_DEFAULT_DEADLINE_MS);
		for (const method of [
			"GET /v1/backup/list",
			"POST /v1/backup/create",
			"POST /v1/backup/verify",
			"POST /v1/backup/restore",
		]) {
			expect(core.rpcDeadlineForMethod(method)).toBeGreaterThan(2_000);
		}

		const oversized = await new Promise<string>((resolve, reject) => {
			void core.startDaemon({ dataDir: tempDataDir() }).then((handle) => {
				created.push(handle);
				const socket = createConnection(handle.socketPath);
				let buf = "";
				socket.once("error", reject);
				socket.once("connect", () => {
					socket.write(`${"x".repeat(core.RPC_MAX_BYTES + 1)}\n`);
				});
				socket.on("data", (chunk) => {
					buf += chunk.toString("utf8");
					if (buf.includes("\n")) {
						socket.destroy();
						resolve(buf);
					}
				});
				socket.setTimeout(2000, () => reject(new Error("size probe timed out")));
			});
		});
		expect(JSON.parse(oversized)).toMatchObject({ error: { code: "payload_too_large" } });

		const oversizedResponse = await new Promise<unknown>((resolve, reject) => {
			const dataDir = tempDataDir();
			const layout = core.resolveStorageLayout(dataDir);
			mkdirSync(layout.controlDir, { recursive: true, mode: 0o700 });
			const server = createServer((socket) => {
				socket.once("data", () => socket.write("x".repeat(core.RPC_MAX_BYTES + 1)));
			});
			server.once("error", reject);
			server.listen(layout.socketPath, () => {
				core
					.callDaemonRpc(layout.socketPath, handshake(), {
						timeoutMs: 1_000,
						maxResponseBytes: core.RPC_MAX_BYTES,
					})
					.then(
						(value) => {
							server.close();
							resolve(value);
						},
						(error: unknown) => {
							server.close();
							resolve(error);
						},
					);
			});
		});
		expect(oversizedResponse).toBeInstanceOf(Error);
		expect(String(oversizedResponse)).toContain("response exceeds");

		const responseWithTrailingBytes = await new Promise<unknown>((resolve, reject) => {
			const dataDir = tempDataDir();
			const layout = core.resolveStorageLayout(dataDir);
			mkdirSync(layout.controlDir, { recursive: true, mode: 0o700 });
			const response = `${JSON.stringify({ id: "req-1", result: { status: "ok" } })}\n`;
			const server = createServer((socket) => {
				socket.once("data", () => socket.end(`${response}${"x".repeat(core.RPC_MAX_BYTES)}`));
			});
			server.once("error", reject);
			server.listen(layout.socketPath, () => {
				core
					.callDaemonRpc(layout.socketPath, handshake(), {
						timeoutMs: 1_000,
						maxResponseBytes: Buffer.byteLength(response),
					})
					.then(
						(value) => {
							server.close();
							resolve(value);
						},
						(error: unknown) => {
							server.close();
							reject(error);
						},
					);
			});
		});
		expect(responseWithTrailingBytes).toEqual({ id: "req-1", result: { status: "ok" } });

		let now = 0;
		const handle = await core.startDaemon({
			dataDir: tempDataDir(),
			rpcDeadlineMs: 50,
			now: () => {
				now += 100;
				return now;
			},
		});
		created.push(handle);
		const late = await core.callDaemonRpc(handle.socketPath, handshake());
		expect(late).toMatchObject({ error: { code: "deadline_exceeded", retryable: true } });

		const restoreDataDir = tempDataDir();
		const restoreLayout = core.resolveStorageLayout(restoreDataDir);
		mkdirSync(restoreLayout.controlDir, { recursive: true, mode: 0o700 });
		let releaseRestore: (() => void) | undefined;
		const restoreGate = new Promise<void>((resolve) => {
			releaseRestore = resolve;
		});
		let stopCalls = 0;
		const restoreServer = createServer((connection) => {
			attachDaemonRpc(connection, {
				identity: { pid: process.pid, nonce: "late-restore" },
				dataDir: restoreDataDir,
				deadlineMs: 10,
				onStop: () => {
					stopCalls++;
				},
				writer: {} as never,
				store: {} as never,
				viewerAuth: {} as never,
				viewerRead: async () => ({}),
				jobs: {
					isMaintenanceMode: () => false,
					hasPendingWork: () => false,
				} as never,
				operations: {
					hasPending: () => false,
					runBackup: async () => {
						await restoreGate;
						return {
							operationId: "late-restore",
							backupId: "late-backup",
							pointer: "versions/late.sqlite",
							artifactSha256: "a".repeat(64),
							manifestHash: "b".repeat(64),
							restartRequired: true,
						};
					},
				} as never,
				restoreState: { active: false },
			});
		});
		await new Promise<void>((resolve, reject) => {
			restoreServer.once("error", reject);
			restoreServer.listen(restoreLayout.socketPath, resolve);
		});
		try {
			const timedOutRestore = await core.callDaemonRpc(
				restoreLayout.socketPath,
				handshake({
					method: "POST /v1/backup/restore",
					body: {
						operationId: "late-restore",
						backupId: "late-backup",
						payloadHash: core.restorePayloadHash("late-backup"),
					},
				}),
				{ timeoutMs: 1_000 },
			);
			expect(timedOutRestore).toMatchObject({ error: { code: "deadline_exceeded" } });
			expect(stopCalls).toBe(0);
			releaseRestore?.();
			for (let attempt = 0; attempt < 50 && stopCalls === 0; attempt++) {
				await new Promise<void>((resolve) => setImmediate(resolve));
			}
			expect(stopCalls).toBe(1);
		} finally {
			releaseRestore?.();
			await new Promise<void>((resolve) => restoreServer.close(() => resolve()));
		}
	});

	it("P1-T035-04-health-doctor", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const health = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "GET /v1/health" }),
		);
		expect(health).toMatchObject({
			id: "req-1",
			result: {
				status: "ok",
				instanceId: handle.identity.nonce,
				protocolVersion: {
					localApi: core.LOCAL_API_VERSION,
					normalizedSchema: core.NORMALIZED_SCHEMA_VERSION,
				},
			},
		});
		const doctor = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "GET /v1/doctor" }),
		);
		expect(doctor).toMatchObject({
			result: {
				status: "ok",
				instanceId: handle.identity.nonce,
				protocolVersion: {
					localApi: core.LOCAL_API_VERSION,
					normalizedSchema: core.NORMALIZED_SCHEMA_VERSION,
				},
				diagnostics: {
					lock: "held",
					socket: "listening",
					hookDelivery: {
						implementation: "node-fallback",
						p95TargetMs: 150,
						budgets: core.HOOK_DELIVERY_BUDGETS,
					},
					redaction: {
						status: "ok",
						degradedDeliveries: 0,
						workerDeadlineMs: core.REDACTION_WORKER_DEADLINE_MS,
					},
				},
			},
		});

		const degradedId = "p1-doctor-degraded";
		expect(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: degradedId,
					method: "POST /v1/events",
					body: {
						idempotencyKey: degradedId,
						event: normalizedEvent(degradedId),
						adapterRedaction: {
							sensitivity: "secret",
							secret_rules_version: `${"a".repeat(64)}:degraded`,
							redaction_degraded: true,
							private_content_omitted: false,
							local_only: false,
						},
					},
				}),
			),
		).toMatchObject({ result: { status: "quarantined" } });
		expect(
			rpcResult(
				await core.callDaemonRpc(
					handle.socketPath,
					handshake({ id: "p1-doctor-warning", method: "GET /v1/doctor" }),
				),
			).diagnostics,
		).toMatchObject({
			redaction: {
				status: "warning",
				degradedDeliveries: 1,
				workerDeadlineMs: core.REDACTION_WORKER_DEADLINE_MS,
			},
		});
	});

	it("rejects malformed memory adapter redaction metadata", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const response = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				method: "POST /v1/memories/record",
				body: {
					idempotencyKey: "malformed-memory-redaction",
					kind: "decision",
					title: "Safe title",
					body: "Safe body",
					adapterRedaction: {},
				},
			}),
		);
		expect(response).toMatchObject({
			error: { code: "invalid_request", message: "adapterRedaction is malformed." },
		});
	});

	it("P1-T043-10-daemon-auth-rpc exchanges and verifies sessions through the daemon", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const nonceResponse = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "POST /v1/viewer/auth/nonce" }),
		);
		if (!("result" in nonceResponse)) throw new Error("nonce RPC failed");
		const nonce = String(nonceResponse.result.nonce);

		const exchange = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "POST /v1/viewer/auth/exchange", body: { nonce } }),
		);
		if (!("result" in exchange)) throw new Error("exchange RPC failed");
		const session = (exchange.result.session as { cookie?: unknown } | null)?.cookie;
		expect(typeof session).toBe("string");

		const verify = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				method: "POST /v1/viewer/auth/verify",
				body: { session },
			}),
		);
		expect(verify).toMatchObject({ result: { authenticated: true } });

		const logout = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "POST /v1/viewer/auth/logout", body: { session } }),
		);
		expect(logout).toMatchObject({ result: { loggedOut: true } });
		const rejected = await core.callDaemonRpc(
			handle.socketPath,
			handshake({ method: "POST /v1/viewer/auth/verify", body: { session } }),
		);
		expect(rejected).toMatchObject({ result: { authenticated: false } });
	});

	it("P1-T043-12-daemon-view-collections serves collections from the daemon store", async () => {
		const dataDir = tempDataDir();
		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const recorded = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				method: "POST /v1/memories/record",
				body: {
					idempotencyKey: "viewer-seed",
					kind: "discovery",
					title: "Viewer seed",
					body: "Visible through daemon RPC",
					project: "/tmp/viewer-project",
				},
			}),
		);
		expect(recorded).toMatchObject({ result: { memoryId: expect.any(Number) } });
		await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "viewer-summary",
				method: "POST /v1/memories/record",
				body: {
					idempotencyKey: "viewer-summary",
					kind: "session_summary",
					title: "Viewer summary",
					body: "Summary through daemon RPC",
					project: "/tmp/viewer-project",
				},
			}),
		);

		const view = async (collection: string, body: Record<string, unknown> = {}) => {
			const response = await core.callDaemonRpc(
				handle.socketPath,
				handshake({ method: "GET /v1/view", body: { collection, ...body } }),
			);
			if (!("result" in response)) throw new Error(`${collection} view failed`);
			return response.result;
		};

		expect(await view("projects")).toEqual({
			status: 200,
			body: { projects: ["viewer-project"] },
		});
		expect(await view("observations", { limit: 10 })).toMatchObject({
			status: 200,
			body: { items: [{ title: "Viewer seed" }], pagination: { has_more: false } },
		});
		expect(await view("summaries", { limit: 10 })).toMatchObject({
			status: 200,
			body: { items: [{ title: "Viewer summary" }], pagination: { has_more: false } },
		});
		const sessions = await view("sessions");
		expect(sessions).toMatchObject({
			status: 200,
			body: { items: [{ project: "/tmp/viewer-project" }] },
		});
		const sessionId = Number((sessions.body as { items: Array<{ id: number }> }).items[0]?.id);
		expect(await view("artifacts", { sessionId })).toEqual({
			status: 200,
			body: { items: [] },
		});
		expect(await view("session", { project: "/tmp/viewer-project" })).toMatchObject({
			status: 200,
			body: { memories: 2, observations: 1 },
		});
		expect(await view("stats")).toMatchObject({
			status: 200,
			body: { database: { memory_items: 2 }, maintenance_jobs: expect.any(Array) },
		});
		expect(await view("runtime")).toEqual({ status: 200, body: { version: core.VERSION } });
		expect(await view("raw-events")).toMatchObject({
			status: 200,
			body: { pending: 0, sessions: 0 },
		});
		expect(await view("raw-events-status")).toMatchObject({
			status: 200,
			body: { items: [], ingest: { available: false, mode: "daemon_rpc" } },
		});
		expect(await view("observer-status")).toMatchObject({
			status: 200,
			body: { queue: { pending: 0, sessions: 0 } },
		});
		const previousConfig = process.env.CODEMEM_CONFIG;
		const previousHeaders = process.env.CODEMEM_OBSERVER_HEADERS;
		const configPath = join(dataDir, "viewer-config.json");
		writeFileSync(configPath, JSON.stringify({ observer_api_key: "viewer-secret" }));
		process.env.CODEMEM_CONFIG = configPath;
		process.env.CODEMEM_OBSERVER_HEADERS = JSON.stringify({ Authorization: "secret-value" });
		try {
			const config = await view("config");
			expect(config).toMatchObject({
				status: 200,
				body: {
					capability: {
						mode: "capture_only",
						configurationFingerprint: null,
						providerEnabled: false,
						lexicalEnabled: true,
					},
				},
			});
			expect(JSON.stringify(config)).not.toContain("viewer-secret");
			expect(JSON.stringify(config)).not.toContain("secret-value");
		} finally {
			if (previousConfig === undefined) delete process.env.CODEMEM_CONFIG;
			else process.env.CODEMEM_CONFIG = previousConfig;
			if (previousHeaders === undefined) delete process.env.CODEMEM_OBSERVER_HEADERS;
			else process.env.CODEMEM_OBSERVER_HEADERS = previousHeaders;
		}
	});

	it("P1-T045-01-job-id-result", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const remembered = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				method: "POST /v1/memories/record",
				body: {
					idempotencyKey: "job-memory",
					kind: "session_summary",
					title: "Job seed",
					body: "## Completed\nMigrated hooks\n\n## Learned\nDaemon owns writes",
				},
			}),
		);
		if (!("result" in remembered)) throw new Error("memory seed failed");

		const submitted = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "submit-job",
				method: "POST /v1/jobs",
				body: { kind: "narrative.backfill", args: { limit: 10 } },
			}),
		);
		if (!("result" in submitted)) throw new Error("job submission failed");
		expect(submitted.result).toMatchObject({
			jobId: expect.stringMatching(/^[0-9a-f-]{36}$/),
			state: "queued",
		});

		const jobId = String(submitted.result.jobId);
		const job = await waitForDaemonJob(handle.socketPath, jobId);
		expect(job).toMatchObject({
			jobId,
			kind: "narrative.backfill",
			state: "completed",
			attempts: 1,
			maxAttempts: 1,
			result: { checked: 1, updated: 1, skipped: 0 },
		});

		const memory = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "job-memory-get",
				method: "GET /v1/memories/:id",
				body: { id: remembered.result.memoryId, requestId: "job-memory-get" },
			}),
		);
		if (!("result" in memory)) throw new Error("memory read failed");
		const sessionId = Number((memory.result.item as { session_id: number }).session_id);
		const layout = core.resolveStorageLayout(handle.dataDir);
		const pointer = core.readCurrentDatabasePointer(layout);
		if (!pointer) throw new Error("canonical database pointer is missing");
		const dbPath = join(layout.dbDir, pointer);
		process.env.CODEMEM_EMBEDDING_DISABLED = "1";
		try {
			const structured = await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: "submit-structured.backfill",
					method: "POST /v1/jobs",
					body: {
						kind: "structured.backfill",
						args: { limit: 10, kinds: ["discovery"], overwrite: false },
					},
				}),
			);
			expect(structured).toMatchObject({
				error: { code: "invalid_request", message: expect.stringMatching(/manifest_absent/i) },
			});
			const vectors = await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: "submit-vectors.migrate",
					method: "POST /v1/jobs",
					body: { kind: "vectors.migrate", args: { batchSize: 10 } },
				}),
			);
			expect(vectors).toMatchObject({
				error: { code: "invalid_request", message: expect.stringMatching(/semantic_disabled/i) },
			});
			for (const [kind, args, dryRun] of [
				["db.vacuum", {}],
				["raw-events.prune", { maxAgeDays: 36_500, vacuum: false }, true],
				["raw-events.retry", { limit: 5 }],
				["projects.rename", { oldName: "missing", newName: "renamed" }, true],
				["projects.normalize", {}, true],
				["observations.prune", { limit: 5 }, true],
				["memories.prune", { limit: 5, kinds: ["observation"] }, true],
				["memories.dedup", { limit: 5, windowMs: 3_600_000 }, true],
				["secrets.scan", { limit: 5 }, true],
				["tags.backfill", { limit: 10 }],
				["dedup-keys.backfill", { limit: 10 }],
				["refs.backfill", { batchSize: 10 }],
				["scopes.backfill", { batchSize: 10 }],
				["session-context.backfill", { batchSize: 10 }],
				["summary-dedup.backfill", { batchSize: 10 }],
				["report.memory-role", { allProjects: true, includeInactive: false, probes: [] }],
				["report.role-compare", { baselineDbPath: dbPath, candidateDbPath: dbPath }],
				["report.artifact", { allProjects: true, includeInactive: false }],
				["report.relink", { allProjects: true, limit: 5 }],
				["plan.relink", { allProjects: true, limit: 5 }],
				[
					"report.extraction",
					{ sessionId, scenarioId: "simple-batch-shape", includeInactive: false },
				],
				["report.raw-events", { limit: 5 }],
				[
					"gate.raw-events",
					{
						minFlushSuccessRate: 0.95,
						maxDroppedEventRate: 0.05,
						minSessionBoundaryAccuracy: 0.9,
						windowHours: 24,
					},
				],
				["report.db-size", { limit: 5 }],
				["db.init", {}],
			] as const) {
				const next = await core.callDaemonRpc(
					handle.socketPath,
					handshake({
						id: `submit-${kind}`,
						method: "POST /v1/jobs",
						body: { kind, args, dryRun: dryRun ?? kind.startsWith("report.") },
					}),
				);
				expect("result" in next, `${kind} submission`).toBe(true);
				if (!("result" in next)) continue;
				const completed = await waitForDaemonJob(handle.socketPath, String(next.result.jobId));
				expect(completed, kind).toMatchObject({
					kind,
					state: "completed",
					attempts: 1,
					maxAttempts: 1,
					result: expect.anything(),
				});
			}
		} finally {
			delete process.env.CODEMEM_EMBEDDING_DISABLED;
		}
	});

	it("P1-T045-03-worker-absorbed", async () => {
		const dataDir = tempDataDir();
		const first = await core.startDaemon({ dataDir });
		created.push(first);
		const remembered = await core.callDaemonRpc(
			first.socketPath,
			handshake({
				method: "POST /v1/memories/record",
				body: {
					idempotencyKey: "legacy-dedup-key",
					kind: "discovery",
					title: "Legacy memory",
					body: "Backfill this row inside the daemon",
				},
			}),
		);
		if (!("result" in remembered)) throw new Error("memory seed failed");
		const memoryId = Number(remembered.result.memoryId);
		await first.stop();

		const layout = core.resolveStorageLayout(dataDir);
		const pointer = core.readCurrentDatabasePointer(layout);
		if (!pointer) throw new Error("canonical database pointer is missing");
		const dbPath = join(layout.dbDir, pointer);
		const seed = connect(dbPath);
		try {
			seed.prepare("UPDATE memory_items SET dedup_key = NULL WHERE id = ?").run(memoryId);
			seed.prepare("DELETE FROM maintenance_jobs WHERE kind = ?").run(core.DEDUP_KEY_BACKFILL_JOB);
		} finally {
			seed.close();
		}

		const second = await core.startDaemon({ dataDir });
		created.push(second);
		let jobs: Array<Record<string, unknown>> = [];
		for (let attempt = 0; attempt < 100; attempt++) {
			const response = await core.callDaemonRpc(
				second.socketPath,
				handshake({
					id: `auto-job-${attempt}`,
					method: "GET /v1/jobs",
					body: { kind: "dedup-keys.backfill" },
				}),
			);
			if ("result" in response) {
				jobs = response.result.jobs as Array<Record<string, unknown>>;
				if (jobs.some((job) => job.state === "completed")) break;
			}
			await new Promise((resolve) => setTimeout(resolve, 10));
		}
		expect(jobs).toMatchObject([
			{
				kind: "dedup-keys.backfill",
				state: "completed",
				attempts: 1,
				maxAttempts: 1,
			},
		]);

		await second.stop();
		const verify = connect(dbPath);
		try {
			const row = verify
				.prepare("SELECT dedup_key FROM memory_items WHERE id = ?")
				.get(memoryId) as { dedup_key: string | null };
			expect(row.dedup_key).toBeTypeOf("string");
		} finally {
			verify.close();
		}
	});

	it("P1-T041-04-file-search stays repository-relative", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const escaped = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				method: "POST /v1/search",
				body: { requestId: "search-escape", mode: "find_by_file", repositoryPath: "../secret" },
			}),
		);
		expect(escaped).toMatchObject({ error: { code: "invalid_request" } });

		const safe = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "req-safe-file",
				method: "POST /v1/search",
				body: {
					requestId: "search-safe",
					mode: "find_by_file",
					repositoryPath: "src/auth.ts",
				},
			}),
		);
		expect(safe).toMatchObject({ result: { items: [], retrievalReceiptId: expect.any(String) } });
	});

	it("applies search filters to get_many reads", async () => {
		const dataDir = tempDataDir();
		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const remember = async (id: string, project: string) => {
			const response = await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id,
					method: "POST /v1/memories/record",
					body: {
						idempotencyKey: id,
						kind: "decision",
						title: `${project} title`,
						body: `${project} body`,
						project,
					},
				}),
			);
			if ("error" in response) throw new Error(response.error.code);
			return Number(response.result.memoryId);
		};
		const demoId = await remember("remember-demo", "demo");
		const otherId = await remember("remember-other", "other");
		const response = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "get-many",
				method: "POST /v1/search",
				body: {
					requestId: "get-many",
					mode: "get_many",
					ids: [demoId, otherId],
					filters: { project: "demo" },
				},
			}),
		);
		expect(response).toMatchObject({ result: { items: [{ id: demoId }] } });

		const mcp = (id: string, method: core.RpcMethod, body: Record<string, unknown>) =>
			core.callDaemonRpc(
				handle.socketPath,
				handshake({ id, method, native_cli_version: "mcp-stdio", body }),
			);
		const successfulReads: Array<[string, string, core.RpcMethod, Record<string, unknown>]> = [
			["mcp-get", "mcp_get", "GET /v1/memories/:id", { id: demoId, project: "demo" }],
			[
				"mcp-get-many",
				"mcp_get_observations",
				"POST /v1/search",
				{ mode: "get_many", ids: [demoId, otherId], filters: { project: "demo" } },
			],
			[
				"mcp-search",
				"mcp_search",
				"POST /v1/search",
				{ mode: "search", query: "demo", limit: 5, filters: { project: "demo" } },
			],
			[
				"mcp-search-index",
				"mcp_search_index",
				"POST /v1/search",
				{ mode: "search_index", query: "demo", limit: 8, filters: { project: "demo" } },
			],
			[
				"mcp-recent",
				"mcp_recent",
				"POST /v1/search",
				{ mode: "recent", limit: 8, filters: { project: "demo" } },
			],
			[
				"mcp-timeline",
				"mcp_timeline",
				"POST /v1/search",
				{
					mode: "timeline",
					memoryId: demoId,
					depthBefore: 0,
					depthAfter: 0,
					filters: { project: "demo" },
				},
			],
			[
				"mcp-explain",
				"mcp_explain",
				"POST /v1/search",
				{ mode: "explain", query: "demo", limit: 10, filters: { project: "demo" } },
			],
			[
				"mcp-expand",
				"mcp_expand",
				"POST /v1/search",
				{
					mode: "expand",
					ids: [demoId],
					depthBefore: 0,
					depthAfter: 0,
					filters: { project: "demo" },
				},
			],
			[
				"mcp-pack",
				"mcp_pack",
				"POST /v1/context/pack",
				{ context: "demo", limit: 5, filters: { project: "demo" } },
			],
		];
		const successes = await Promise.all(
			successfulReads.map(([requestId, , method, body]) =>
				mcp(requestId, method, { requestId, ...body }),
			),
		);
		const attemptIds = successes.map((success) => {
			if ("error" in success) throw new Error(success.error.code);
			expect(success.result.retrievalAttemptId).toMatch(
				/^[0-9a-f]{8}-[0-9a-f]{4}-8[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
			);
			return String(success.result.retrievalAttemptId);
		});
		const replay = await mcp("mcp-search", "POST /v1/search", {
			requestId: "mcp-search",
			mode: "search",
			query: "demo",
			limit: 5,
			filters: { project: "demo" },
		});
		expect(replay).toMatchObject({ result: { retrievalAttemptId: attemptIds[2] } });
		const activeLayout = core.resolveStorageLayout(dataDir);
		const activePointer = core.readCurrentDatabasePointer(activeLayout);
		const pendingReader = ReadOnlyActor.open(resolve(activeLayout.dbDir, activePointer as string));
		try {
			expect(
				pendingReader
					.prepare("SELECT DISTINCT delivery_status FROM retrieval_attempts WHERE source = 'mcp'")
					.pluck()
					.all(),
			).toEqual(["not_attempted"]);
		} finally {
			pendingReader.close();
		}
		for (const attemptId of attemptIds) {
			expect(
				await mcp(`delivery-${attemptId}`, "POST /v1/retrieval/file-context/delivery", {
					attemptId,
					status: "handed_off",
				}),
			).toMatchObject({ result: { updated: true } });
		}
		const empty = await mcp("mcp-empty", "POST /v1/search", {
			requestId: "mcp-empty",
			mode: "search",
			query: "no-such-retrieval-result",
			limit: 5,
			filters: { project: "demo" },
		});
		expect(empty).toMatchObject({ result: { items: [] } });
		const failed = await mcp("mcp-failed", "POST /v1/search", {
			requestId: "mcp-failed",
			mode: "explain",
			limit: 10,
			filters: { project: "demo" },
		});
		expect(failed).toMatchObject({
			result: { items: { errors: [{ code: "INVALID_ARGUMENT", field: "query" }] } },
		});
		const layout = core.resolveStorageLayout(dataDir);
		const pointer = core.readCurrentDatabasePointer(layout);
		const faultDb = connect(resolve(layout.dbDir, pointer as string));
		try {
			faultDb.exec(`
				CREATE TRIGGER fail_mcp_retrieval_attempt_insert
				BEFORE INSERT ON retrieval_attempts
				BEGIN
					SELECT RAISE(ABORT, 'injected MCP retrieval ledger failure');
				END;
			`);
		} finally {
			faultDb.close();
		}
		const getWithBrokenLedger = await mcp("mcp-ledger-broken-get", "GET /v1/memories/:id", {
			id: demoId,
			requestId: "mcp-ledger-broken-get",
			project: "demo",
		});
		expect(getWithBrokenLedger).toMatchObject({ result: { item: { id: demoId } } });
		if ("result" in getWithBrokenLedger) {
			expect(getWithBrokenLedger.result).not.toHaveProperty("retrievalAttemptId");
		}
		const searchErrorWithBrokenLedger = await mcp("mcp-ledger-broken-search", "POST /v1/search", {
			requestId: "mcp-ledger-broken-search",
			mode: "search",
		});
		expect(searchErrorWithBrokenLedger).toMatchObject({
			error: { code: "invalid_request", message: "query is required for search mode." },
		});

		await handle.stop();
		const store = openTestMemoryStore(resolve(layout.dbDir, pointer as string));
		try {
			type Attempt = {
				attempt_id: string;
				request_id: string;
				surface: string;
				retrieval_status: string;
				delivery_status: string;
				candidate_count: number;
				selected_count: number;
				failure_code: string | null;
				failure_stage: string | null;
			};
			const attempts = store.db
				.prepare(
					`SELECT attempt_id, request_id, surface, retrieval_status, delivery_status,
					        candidate_count, selected_count, failure_code, failure_stage
					 FROM retrieval_attempts WHERE source = 'mcp' ORDER BY request_id`,
				)
				.all() as Attempt[];
			expect(attempts).toHaveLength(11);
			expect(
				attempts.every((attempt) =>
					/^[0-9a-f]{8}-[0-9a-f]{4}-8[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(
						attempt.attempt_id,
					),
				),
			).toBe(true);
			const byRequest = new Map(attempts.map((attempt) => [attempt.request_id, attempt]));
			for (const [requestId, surface] of successfulReads) {
				expect(byRequest.get(requestId)).toMatchObject({
					surface,
					retrieval_status: "succeeded",
					delivery_status: "handed_off",
					candidate_count: 1,
					selected_count: 1,
					failure_code: null,
					failure_stage: null,
				});
			}
			expect(byRequest.get("mcp-empty")).toMatchObject({
				surface: "mcp_search",
				retrieval_status: "no_results",
				delivery_status: "not_attempted",
				candidate_count: 0,
				selected_count: 0,
				failure_code: null,
				failure_stage: null,
			});
			expect(byRequest.get("mcp-failed")).toMatchObject({
				surface: "mcp_explain",
				retrieval_status: "failed",
				delivery_status: "not_attempted",
				candidate_count: 0,
				selected_count: 0,
				failure_code: "tool_failed",
				failure_stage: "retrieval",
			});
			const exposures = store.db
				.prepare(
					`SELECT exposures.memory_id, exposures.disposition, exposures.handoff_status
					 FROM retrieval_exposures AS exposures
					 JOIN retrieval_attempts AS attempts USING (attempt_id)
					 WHERE attempts.source = 'mcp'`,
				)
				.all() as Array<{
				memory_id: number;
				disposition: string;
				handoff_status: string;
			}>;
			expect(exposures).toHaveLength(9);
			expect(
				exposures.every(
					(exposure) =>
						exposure.memory_id === demoId &&
						exposure.disposition === "selected" &&
						exposure.handoff_status === "handed_off",
				),
			).toBe(true);
			expect(
				store.db
					.prepare("SELECT count(*) FROM retrieval_attempts WHERE request_id = 'get-many'")
					.pluck()
					.get(),
			).toBe(0);
		} finally {
			store.close();
		}
	});

	it("P1-T041-05 records and completes the file-context retrieval ledger in the daemon", async () => {
		const dataDir = tempDataDir();
		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const common = {
			startedAt: "2026-08-14T01:00:00.000Z",
			completedAt: "2026-08-14T01:00:00.010Z",
			repositoryPath: "src/auth.ts",
			project: "free-mem",
			sourceSessionId: "claude-session",
		};
		const noResults = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "req-ledger-empty",
				method: "POST /v1/retrieval/file-context",
				body: {
					...common,
					attemptId: "11111111-1111-4111-8111-111111111111",
					retrievalStatus: "no_results",
				},
			}),
		);
		expect(noResults).toMatchObject({ result: { recorded: true } });

		const succeeded = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "req-ledger-success",
				method: "POST /v1/retrieval/file-context",
				body: {
					...common,
					attemptId: "22222222-2222-4222-8222-222222222222",
					retrievalStatus: "succeeded",
					candidateIds: [999],
					candidateCount: 1,
					selectedIds: [999],
				},
			}),
		);
		expect(succeeded).toMatchObject({ result: { recorded: true } });
		const delivered = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "req-ledger-delivery",
				method: "POST /v1/retrieval/file-context/delivery",
				body: {
					attemptId: "22222222-2222-4222-8222-222222222222",
					status: "handed_off",
				},
			}),
		);
		expect(delivered).toMatchObject({ result: { updated: true } });

		const escaped = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "req-ledger-escape",
				method: "POST /v1/retrieval/file-context",
				body: {
					...common,
					attemptId: "33333333-3333-4333-8333-333333333333",
					repositoryPath: "/secret",
					retrievalStatus: "no_results",
				},
			}),
		);
		expect(escaped).toMatchObject({ error: { code: "invalid_request" } });

		await handle.stop();
		const layout = core.resolveStorageLayout(dataDir);
		const pointer = core.readCurrentDatabasePointer(layout);
		const store = openTestMemoryStore(resolve(layout.dbDir, pointer as string));
		try {
			const rows = store.db
				.prepare(
					"SELECT attempt_id, surface, retrieval_status, delivery_status FROM retrieval_attempts ORDER BY attempt_id",
				)
				.all();
			expect(rows).toEqual([
				{
					attempt_id: "11111111-1111-4111-8111-111111111111",
					surface: "file_context",
					retrieval_status: "no_results",
					delivery_status: "not_attempted",
				},
				{
					attempt_id: "22222222-2222-4222-8222-222222222222",
					surface: "file_context",
					retrieval_status: "succeeded",
					delivery_status: "handed_off",
				},
			]);
		} finally {
			store.close();
		}
	});
});

describe("T008 capability runtime RPC", () => {
	it("starts without current in capture-only mode with lexical RPC and no provider or sweeper", async () => {
		const handle = await core.startDaemon({ dataDir: tempDataDir() });
		created.push(handle);
		const event = normalizedEvent("t008-absent-event");
		const captured = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "t008-absent-capture",
				method: "POST /v1/events",
				body: { idempotencyKey: event.idempotencyKey, event },
			}),
		);
		expect(rpcResult(captured)).toMatchObject({ receiptId: expect.any(String) });
		const lexical = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "t008-absent-lexical",
				method: "POST /v1/search",
				body: { requestId: "t008-absent-lexical", mode: "search", query: "t008" },
			}),
		);
		expect(rpcResult(lexical)).toMatchObject({ items: expect.any(Array) });
		const doctor = doctorCapability(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake({ id: "t008-absent-doctor", method: "GET /v1/doctor" }),
			),
		);
		expect(Reflect.get(doctor, "mode") ?? Reflect.get(doctor, "state")).toBe("capture_only");
		expect(Reflect.get(doctor, "configurationFingerprint")).toBeNull();
		expect(featureEnabled(doctor, "provider")).toBe(false);
		expect(featureEnabled(doctor, "sweeper")).toBe(false);
		expect(featureEnabled(doctor, "lexical")).toBe(true);
	});

	it("freezes one valid manifest and reports pending privacy identity after current changes", async () => {
		const dataDir = tempDataDir();
		const first = capabilityManifest(
			"http://127.0.0.1:1234/v1/chat/completions",
			"t008-first-model",
		);
		activateManifest(dataDir, first);
		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const second = capabilityManifest(
			"http://127.0.0.1:1234/v1/chat/completions",
			"t008-second-model",
			first.configurationFingerprint,
		);
		activateManifest(dataDir, second);

		for (const requestId of ["t008-valid-doctor-1", "t008-valid-doctor-2"]) {
			const capability = doctorCapability(
				await core.callDaemonRpc(
					handle.socketPath,
					handshake({ id: requestId, method: "GET /v1/doctor" }),
				),
			);
			expect(capability).toMatchObject({
				configurationFingerprint: first.configurationFingerprint,
				runtimeReason: "pending_privacy_boundary",
				summaryProvider: { providerFingerprint: first.summaryProvider.providerFingerprint },
			});
			expect(featureEnabled(capability, "provider")).toBe(false);
			expect(featureEnabled(capability, "sweeper")).toBe(false);
		}
	});

	it("keeps writer, RPC, capture, spool, and lexical ready when daemon TLS preflight degrades provider", async () => {
		const received: Buffer[] = [];
		const server = await new Promise<import("node:net").Server>((resolveServer) => {
			const listener = createServer((socket) => {
				socket.on("data", (chunk) => {
					received.push(Buffer.from(chunk));
					socket.destroy();
				});
			});
			listener.listen(0, "127.0.0.1", () => resolveServer(listener));
		});
		const address = server.address();
		if (!address || typeof address === "string") throw new Error("expected TCP test address");
		const dataDir = tempDataDir();
		const manifest = capabilityManifest(
			`https://127.0.0.1:${address.port}/v1/chat/completions`,
			"t008-tls-model",
		);
		activateManifest(dataDir, manifest);
		let handle: Awaited<ReturnType<typeof core.startDaemon>> | undefined;
		try {
			handle = await core.startDaemon({ dataDir });
			created.push(handle);
		} finally {
			await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
		}
		if (!handle) throw new Error("daemon must survive provider TLS failure");

		const event = normalizedEvent("t008-tls-event");
		const captured = rpcResult(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: "t008-tls-capture",
					method: "POST /v1/events",
					body: { idempotencyKey: event.idempotencyKey, event },
				}),
			),
		);
		expect(captured).toMatchObject({ receiptId: expect.any(String) });
		const health = rpcResult(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake({ id: "t008-tls-health", method: "GET /v1/health" }),
			),
		);
		expect(health).toMatchObject({ status: "ok", spool: { status: expect.any(String) } });
		const lexical = rpcResult(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: "t008-tls-lexical",
					method: "POST /v1/search",
					body: { requestId: "t008-tls-lexical", mode: "search", query: "t008" },
				}),
			),
		);
		expect(lexical).toMatchObject({ items: expect.any(Array) });
		const capability = doctorCapability(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake({ id: "t008-tls-doctor", method: "GET /v1/doctor" }),
			),
		);
		expect(capability).toMatchObject({
			configurationFingerprint: manifest.configurationFingerprint,
			runtimeReason: "provider_unavailable",
			providerHealth: "provider_unavailable",
			summaryProvider: { providerFingerprint: manifest.summaryProvider.providerFingerprint },
		});
		expect(featureEnabled(capability, "provider")).toBe(false);
		expect(featureEnabled(capability, "lexical")).toBe(true);
		expect(received.length).toBeGreaterThan(0);
	});

	it("reports a rejected TLS trust override as the runtime reason", async () => {
		const dataDir = tempDataDir();
		const manifest = capabilityManifest(
			"https://summary.stub.invalid/v1/chat/completions",
			"t008-trust-override-model",
		);
		activateManifest(dataDir, manifest);
		vi.stubEnv("NODE_TLS_REJECT_UNAUTHORIZED", "0");
		try {
			const handle = await core.startDaemon({ dataDir });
			created.push(handle);
			const capability = doctorCapability(
				await core.callDaemonRpc(
					handle.socketPath,
					handshake({ id: "t008-trust-override-doctor", method: "GET /v1/doctor" }),
				),
			);
			expect(capability).toMatchObject({
				runtimeReason: "provider_tls_rejected",
				providerHealth: "provider_tls_rejected",
			});
		} finally {
			vi.unstubAllEnvs();
		}
	});
});

describe("T018/T019 durable capture and processing-job RPC", () => {
	it("T028 derives repository authority from cwd while labels stay non-authoritative", async () => {
		const dataDir = tempDataDir();
		const repository = join(resolve(dataDir, ".."), "repository");
		mkdirSync(repository);
		execFileSync("git", ["-C", repository, "init", "--quiet"]);
		execFileSync("git", [
			"-C",
			repository,
			"remote",
			"add",
			"origin",
			"https://git.example.invalid/acme/project.git",
		]);
		const expectedIdentity = resolveRepositoryIdentity(repository);
		expect(expectedIdentity).toMatch(/^repo-v1:sha256:[a-f0-9]{64}$/);

		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const idempotencyKey = "t028-repository-authority";
		const event = {
			...normalizedEvent(idempotencyKey),
			idempotencyKey,
			cwd: repository,
			projectKey: "forged-project-label",
			workspaceKey: "forged-workspace-label",
		};
		expect(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: idempotencyKey,
					method: "POST /v1/events",
					body: { idempotencyKey, event },
				}),
			),
		).toMatchObject({ result: { status: "committed" } });

		const activePointer = core.readCurrentDatabasePointer(handle.layout);
		const reader = ReadOnlyActor.open(resolve(handle.layout.dbDir, activePointer as string));
		try {
			expect(
				reader
					.prepare("SELECT repository_identity FROM raw_events WHERE event_id = ?")
					.get(idempotencyKey),
			).toEqual({ repository_identity: expectedIdentity });
		} finally {
			reader.close();
		}
	});

	it("derives the record RPC's repository identity from the caller's cwd", async () => {
		const dataDir = tempDataDir();
		const repository = join(resolve(dataDir, ".."), "record-repository");
		mkdirSync(repository);
		execFileSync("git", ["-C", repository, "init", "--quiet"]);
		const expectedIdentity = resolveRepositoryIdentity(repository);
		expect(expectedIdentity).toMatch(/^repo-v1:sha256:[a-f0-9]{64}$/);

		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const record = (idempotencyKey: string, cwd?: string) =>
			core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: idempotencyKey,
					method: "POST /v1/memories/record",
					body: {
						idempotencyKey,
						kind: "discovery",
						title: `Title ${idempotencyKey}`,
						body: `Body ${idempotencyKey}`,
						...(cwd ? { cwd } : {}),
					},
				}),
			);

		expect(await record("record-cwd-identity", repository)).toMatchObject({
			result: { memoryId: expect.any(Number) },
		});
		expect(await record("record-no-cwd")).toMatchObject({
			result: { memoryId: expect.any(Number) },
		});

		const activePointer = core.readCurrentDatabasePointer(handle.layout);
		const reader = ReadOnlyActor.open(resolve(handle.layout.dbDir, activePointer as string));
		try {
			const identityOf = (title: string) =>
				reader.prepare("SELECT repository_identity FROM memory_items WHERE title = ?").get(title);
			// Gate passes: a git cwd binds the memory to its repository identity.
			expect(identityOf("Title record-cwd-identity")).toEqual({
				repository_identity: expectedIdentity,
			});
			// Gate fires: no probe input, no identity — never inferred elsewhere.
			expect(identityOf("Title record-no-cwd")).toEqual({ repository_identity: null });
		} finally {
			reader.close();
		}
	});

	it("binds record idempotency to the derived repository identity", async () => {
		const dataDir = tempDataDir();
		const repositoryA = join(resolve(dataDir, ".."), "idem-repo-a");
		const repositoryB = join(resolve(dataDir, ".."), "idem-repo-b");
		for (const repo of [repositoryA, repositoryB]) {
			mkdirSync(repo);
			execFileSync("git", ["-C", repo, "init", "--quiet"]);
		}

		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const record = (cwd: string) =>
			core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: `idem-${cwd.endsWith("a") ? "a" : "b"}`,
					method: "POST /v1/memories/record",
					body: {
						idempotencyKey: "same-key-two-repos",
						kind: "discovery",
						title: "Idempotent title",
						body: "Idempotent body",
						cwd,
					},
				}),
			);

		const first = await record(repositoryA);
		expect(first).toMatchObject({ result: { memoryId: expect.any(Number) } });
		// Gate passes: replay from the same repository returns the same receipt.
		const replay = await record(repositoryA);
		expect(replay).toEqual(first);
		// Gate fires: the same key from ANOTHER repository must not replay the
		// first repository's receipt.
		const crossed = await record(repositoryB);
		if ("result" in crossed) {
			expect(crossed).not.toEqual(first);
		} else {
			expect(crossed).toHaveProperty("error");
		}
	});

	it("keeps capture conflicts non-successful and saturates direct admission at two", async () => {
		const dataDir = tempDataDir();
		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const send = async (idempotencyKey: string, event: Record<string, unknown>) =>
			core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: `t018-${idempotencyKey}`,
					method: "POST /v1/events",
					body: { idempotencyKey, event: { ...event, idempotencyKey } },
				}),
			);

		const unknownCwd = join(resolve(dataDir, ".."), "not-a-repository");
		mkdirSync(unknownCwd);
		const canonical = { ...normalizedEvent("t018-null-identity"), cwd: unknownCwd };
		expect(await send("t018-null-identity", canonical)).toMatchObject({
			result: { status: "committed" },
		});
		const conflicting = {
			...canonical,
			cwd: join(unknownCwd, "conflicting"),
			projectKey: "conflicting-project",
			occurredAt: new Date(1_000).toISOString(),
			payload: { text: "must not become canonical" },
			sourceHash: "c".repeat(64),
		};
		const quarantined = await send("t018-null-identity", conflicting);
		const replay = await send("t018-null-identity", conflicting);
		const quarantineReceipt = rpcResult(quarantined);
		const replayReceipt = rpcResult(replay);
		expect(quarantineReceipt).toMatchObject({
			status: "quarantined",
			receiptId: expect.any(String),
			safeErrorCode: "repository_identity_unknown_collision",
		});
		expect(quarantineReceipt.status).not.toBe("committed");
		expect(quarantineReceipt).not.toHaveProperty("ack");
		expect(replayReceipt).toMatchObject({
			status: "quarantined",
			receiptId: quarantineReceipt.receiptId,
		});
		const activePointer = core.readCurrentDatabasePointer(handle.layout);
		const reader = ReadOnlyActor.open(resolve(handle.layout.dbDir, activePointer as string));
		try {
			expect(
				reader
					.prepare(
						`SELECT cwd, project, last_seen_ts_wall_ms
						 FROM raw_event_sessions WHERE source = ? AND stream_id = ?`,
					)
					.get("codex", "t008-session"),
			).toEqual({ cwd: unknownCwd, project: "t008-project", last_seen_ts_wall_ms: 0 });
			expect(
				reader
					.prepare(
						`SELECT COUNT(*) AS count FROM mutation_quarantine
						 WHERE method = ? AND idempotency_key = ?`,
					)
					.get("POST /v1/events", "t018-null-identity"),
			).toEqual({ count: 1 });
		} finally {
			reader.close();
		}

		{
			const items = [0, 1, 2].map((index) => {
				const id = `t018-capture-${index}`;
				const event = normalizedEvent(id);
				return { idempotencyKey: id, event };
			});
			const batch = await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: "t018-capture-batch",
					method: "POST /v1/events/batch",
					body: { items },
				}),
			);
			expect(batch).toMatchObject({
				result: {
					receipts: [{ status: "committed" }, { status: "committed" }, { status: "committed" }],
				},
			});
			expect(
				rpcResult(
					await core.callDaemonRpc(
						handle.socketPath,
						handshake({
							id: "t018-capture-count",
							method: "GET /v1/view",
							body: { collection: "raw-events" },
						}),
					),
				).body,
			).toMatchObject({ pending: 4 });

			const admissionDataDir = tempDataDir();
			const admissionStore = openTestMemoryStore(join(admissionDataDir, "admission.sqlite"));
			try {
				const admissionContext = {
					identity: { pid: process.pid, nonce: "t018-saturated" },
					dataDir: admissionDataDir,
					onStop: () => undefined,
					writer: admissionStore.db,
					store: admissionStore,
					jobs: { isMaintenanceMode: () => false },
					capability: core.captureOnlyCapabilityProjection("ready"),
					captureInFlight: 0,
				} as unknown as Parameters<typeof core.dispatchDaemonRpc>[1];
				const requests = [
					handshake({
						id: "direct-event",
						method: "POST /v1/events",
						body: {
							idempotencyKey: "direct-event",
							event: normalizedEvent("direct-event"),
						},
					}),
					handshake({
						id: "batch-event",
						method: "POST /v1/events/batch",
						body: {
							items: [
								{
									idempotencyKey: "batch-event",
									event: normalizedEvent("batch-event"),
								},
							],
						},
					}),
					handshake({
						id: "rejected-event",
						method: "POST /v1/events",
						body: {
							idempotencyKey: "rejected-event",
							event: normalizedEvent("rejected-event"),
						},
					}),
				];
				const admissions = await Promise.all(
					requests.map((request) =>
						core.dispatchDaemonRpc(JSON.stringify(request), admissionContext),
					),
				);
				expect(admissions[0]).toMatchObject({ result: { status: "committed" } });
				expect(admissions[1]).toMatchObject({
					result: { receipts: [{ status: "committed" }] },
				});
				expect(admissions[2]).toMatchObject({
					error: { code: "capture_saturated", retryable: true },
				});
				expect(admissionContext.captureInFlight).toBe(0);
			} finally {
				admissionStore.close();
			}
		}
	});

	it("recovers one exact legacy-unknown job and rejects stale or concurrent doctor confirmations", async () => {
		const dataDir = tempDataDir();
		const initial = await core.startDaemon({ dataDir });
		await initial.stop();
		const layout = core.resolveStorageLayout(dataDir);
		const pointer = core.readCurrentDatabasePointer(layout);
		if (!pointer) throw new Error("expected canonical database pointer");
		const manifest = capabilityManifest(
			"http://127.0.0.1:1234/v1/chat/completions",
			"t019-legacy-doctor",
		);
		activateManifest(dataDir, manifest);
		const store = openTestMemoryStore(resolve(layout.dbDir, pointer));
		let batchId: number;
		const manifestFingerprint = manifest.configurationFingerprint;
		const providerFingerprint = manifest.summaryProvider.providerFingerprint;
		try {
			store.recordRawEvent({
				opencodeSessionId: "t019-processing-job",
				source: "codex",
				eventId: "t019-processing-job-0",
				eventType: "user_prompt",
				payload: { text: "legacy recovery source" },
				repositoryIdentity: `repo-v1:sha256:${"a".repeat(64)}`,
				sensitivity: "eligible",
			});
			const result = store.db
				.prepare(
					`INSERT INTO raw_event_flush_batches(
						source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
						extractor_version, status, admission_manifest_fingerprint,
						admission_provider_fingerprint, attempt_manifest_fingerprint,
						attempt_provider_fingerprint, retry_limit, attempt_count,
						claim_generation, legacy_recovery_state, created_at, updated_at
					 ) VALUES ('codex', 't019-processing-job', 't019-processing-job', 0, 0,
						'raw_events_v1', 'retry_exhausted', NULL, NULL, NULL, NULL, 3, 3, 0,
						'complete_range', ?, ?)`,
				)
				.run("2026-08-31T00:00:00.000Z", "2026-08-31T00:00:00.000Z");
			batchId = Number(result.lastInsertRowid);
		} finally {
			store.close();
		}
		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const get = () =>
			core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: `t019-get-${batchId}`,
					method: "GET /v1/processing-jobs/:id",
					body: { id: String(batchId) },
				}),
			);
		const before = rpcResult(await get());
		expect(before).toEqual({
			job: {
				jobId: batchId,
				component: "summary",
				state: "retry_exhausted",
				admission: {
					manifestFingerprint: null,
					providerFingerprint: null,
					retryLimit: 3,
				},
				attempt: {
					count: 3,
					claimGeneration: 0,
					manifestFingerprint: null,
					providerFingerprint: null,
					fingerprint: null,
				},
				resume: { grantState: "none", lastSequence: 0 },
				retryTarget: { manifestFingerprint, providerFingerprint },
				nextAction: "confirm_retry",
			},
		});
		expect(JSON.stringify(before)).not.toContain("payload");
		const stale = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: `t019-stale-${batchId}`,
				method: "POST /v1/processing-jobs/:id/doctor-retry",
				body: {
					id: String(batchId),
					producerReceiptId: "doctor-receipt-stale",
					expectedRole: "summary",
					expectedProviderFingerprint: null,
					expectedManifestFingerprint: null,
					expectedAttemptCount: 3,
					expectedClaimGeneration: 1,
				},
			}),
		);
		expect(stale).toMatchObject({ error: { code: "stale_snapshot", retryable: false } });
		expect(rpcResult(await get())).toEqual(before);

		const acceptedRequest = handshake({
			id: `t019-accepted-${batchId}`,
			method: "POST /v1/processing-jobs/:id/doctor-retry",
			body: {
				id: String(batchId),
				producerReceiptId: "doctor-receipt-accepted",
				expectedRole: "summary",
				expectedProviderFingerprint: null,
				expectedManifestFingerprint: null,
				expectedAttemptCount: 3,
				expectedClaimGeneration: 0,
			},
		});
		const accepted = await core.callDaemonRpc(handle.socketPath, acceptedRequest);
		expect(accepted).toMatchObject({
			result: { jobId: batchId, disposition: "accepted", grantState: "pending" },
		});
		const acceptedResult = rpcResult(accepted);
		const duplicate = await core.callDaemonRpc(handle.socketPath, acceptedRequest);
		expect(rpcResult(duplicate)).toEqual({
			jobId: batchId,
			signalId: acceptedResult.signalId,
			producerReceiptId: "doctor-receipt-accepted",
			sequence: 1,
			disposition: "duplicate",
			grantState: "pending",
		});
		const pendingSnapshot = rpcResult(await get());
		const pending = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: `t019-pending-${batchId}`,
				method: "POST /v1/processing-jobs/:id/doctor-retry",
				body: {
					id: String(batchId),
					producerReceiptId: "doctor-receipt-pending",
					expectedRole: "summary",
					expectedProviderFingerprint: null,
					expectedManifestFingerprint: null,
					expectedAttemptCount: 3,
					expectedClaimGeneration: 0,
				},
			}),
		);
		expect(pending).toMatchObject({ error: { code: "grant_pending", retryable: true } });
		expect(rpcResult(await get())).toEqual(pendingSnapshot);

		await handle.stop();
		created.pop();
		rmSync(layout.capabilityCurrentPointerPath);
		const captureOnly = await core.startDaemon({ dataDir });
		created.push(captureOnly);
		const captureOnlySnapshot = rpcResult(
			await core.callDaemonRpc(
				captureOnly.socketPath,
				handshake({
					id: `t019-capture-only-get-${batchId}`,
					method: "GET /v1/processing-jobs/:id",
					body: { id: String(batchId) },
				}),
			),
		);
		expect(captureOnlySnapshot).toMatchObject({
			job: { resume: { grantState: "pending" }, retryTarget: null },
		});
		expect(rpcResult(await core.callDaemonRpc(captureOnly.socketPath, acceptedRequest))).toEqual({
			jobId: batchId,
			signalId: acceptedResult.signalId,
			producerReceiptId: "doctor-receipt-accepted",
			sequence: 1,
			disposition: "duplicate",
			grantState: "pending",
		});
		const unavailableTarget = await core.callDaemonRpc(
			captureOnly.socketPath,
			handshake({
				id: `t019-capture-only-new-${batchId}`,
				method: "POST /v1/processing-jobs/:id/doctor-retry",
				body: {
					...(acceptedRequest.body ?? {}),
					producerReceiptId: "doctor-receipt-capture-only-new",
				},
			}),
		);
		expect(unavailableTarget).toMatchObject({
			error: { code: "stale_snapshot", retryable: false },
		});
		expect(
			rpcResult(
				await core.callDaemonRpc(
					captureOnly.socketPath,
					handshake({
						id: `t019-capture-only-get-after-${batchId}`,
						method: "GET /v1/processing-jobs/:id",
						body: { id: String(batchId) },
					}),
				),
			),
		).toEqual(captureOnlySnapshot);
		await captureOnly.stop();
		created.pop();
		const resumed = openTestMemoryStore(resolve(layout.dbDir, pointer));
		try {
			const claim = resumed.claimRawEventFlushJob({
				jobId: batchId,
				manifestFingerprint,
				providerFingerprint,
				manifest,
				boundary: compileProviderDestinationBoundary(manifest, {
					repositoryIdentity: resumed.rawEventFlushJobRepositoryIdentity(batchId),
					tlsPeerVerified: false,
				}),
			});
			if (!claim) throw new Error("expected legacy doctor claim");
			expect(
				resumed.db
					.prepare(
						`SELECT admission_manifest_fingerprint, admission_provider_fingerprint,
							attempt_manifest_fingerprint, attempt_provider_fingerprint, attempt_fingerprint
						 FROM raw_event_flush_batches WHERE id = ?`,
					)
					.get(batchId),
			).toEqual({
				admission_manifest_fingerprint: null,
				admission_provider_fingerprint: null,
				attempt_manifest_fingerprint: manifestFingerprint,
				attempt_provider_fingerprint: providerFingerprint,
				attempt_fingerprint: claim.attemptFingerprint,
			});
		} finally {
			resumed.close();
		}
	});
});

describe("T031 daemon read boundary", () => {
	it("forces RPC callers to remote authority and exposes only eligible memory", async () => {
		const dataDir = tempDataDir();
		const initialized = await core.startDaemon({ dataDir });
		await initialized.stop();
		const layout = core.resolveStorageLayout(dataDir);
		const pointer = core.readCurrentDatabasePointer(layout);
		if (!pointer) throw new Error("expected canonical database pointer");
		const store = openTestMemoryStore(resolve(layout.dbDir, pointer));
		const repositoryA = `repo-v1:sha256:${"a".repeat(64)}`;
		const repositoryB = `repo-v1:sha256:${"b".repeat(64)}`;
		const sessionId = store.startSession({ project: "forged-project" });
		const insert = (
			title: string,
			sensitivity: "eligible" | "private" | "local_only" | "secret",
			repositoryIdentity: string | null,
			createdAt: string,
		): number => {
			const id = store.remember(sessionId, "discovery", title, "t031 privacy boundary");
			store.db
				.prepare(
					`UPDATE memory_items
					 SET sensitivity = ?, repository_identity = ?, created_at = ?, updated_at = ?
					 WHERE id = ?`,
				)
				.run(sensitivity, repositoryIdentity, createdAt, createdAt, id);
			return id;
		};
		const ids = {
			eligible: insert("Eligible", "eligible", repositoryA, "2026-01-01T00:00:00.000Z"),
			private: insert("Private", "private", repositoryA, "2026-01-01T01:00:00.000Z"),
			localOnly: insert("Local only", "local_only", repositoryA, "2026-01-01T02:00:00.000Z"),
			secret: insert("Secret", "secret", repositoryA, "2026-01-01T03:00:00.000Z"),
			crossRepository: insert(
				"Cross repository",
				"private",
				repositoryB,
				"2026-01-01T04:00:00.000Z",
			),
			unknownRepository: insert("Unknown repository", "private", null, "2026-01-01T05:00:00.000Z"),
		};
		store.close();

		const handle = await core.startDaemon({ dataDir });
		created.push(handle);
		const rpc = async (
			id: string,
			method: core.RpcMethod,
			body: Record<string, unknown>,
		): Promise<Record<string, unknown>> =>
			rpcResult(
				await core.callDaemonRpc(
					handle.socketPath,
					handshake({ id, method, body: { requestId: id, ...body } }),
				),
			);
		const itemIds = (value: unknown): number[] =>
			Array.isArray(value)
				? value
						.map((item) => (item && typeof item === "object" ? Reflect.get(item, "id") : null))
						.filter((id): id is number => typeof id === "number")
						.sort((left, right) => left - right)
				: [];

		for (const [name, id] of Object.entries(ids)) {
			const result = await rpc(`get-${name}`, "GET /v1/memories/:id", {
				id,
				project: "forged-project",
			});
			if (name === "eligible") expect(result.item).toMatchObject({ id: ids.eligible });
			else expect(result.item, name).toBeNull();
		}
		const search = await rpc("search", "POST /v1/search", {
			mode: "search",
			query: "t031 privacy boundary",
			limit: 20,
			filters: { project: "forged-project" },
		});
		const recent = await rpc("recent", "POST /v1/search", {
			mode: "recent",
			limit: 20,
			filters: { project: "forged-project" },
		});
		const timeline = await rpc("timeline", "POST /v1/search", {
			mode: "timeline",
			memoryId: ids.eligible,
			depthBefore: 20,
			depthAfter: 20,
			filters: { project: "forged-project" },
		});
		const explain = await rpc("explain", "POST /v1/search", {
			mode: "explain",
			query: "t031 privacy boundary",
			ids: Object.values(ids),
			limit: 20,
			filters: { project: "forged-project" },
		});
		const pack = await rpc("pack", "POST /v1/context/pack", {
			context: "t031 privacy boundary",
			limit: 20,
			filters: { project: "forged-project" },
		});
		expect(itemIds(search.items)).toEqual([ids.eligible]);
		expect(itemIds(recent.items)).toEqual([ids.eligible]);
		expect(itemIds(timeline.items)).toEqual([ids.eligible]);
		expect(itemIds(Reflect.get(explain.items as object, "items"))).toEqual([ids.eligible]);
		expect(Reflect.get(pack.pack as object, "item_ids")).toEqual([ids.eligible]);

		for (const forgedField of [
			"executionLocation",
			"modelLocal",
			"providerPeerTrust",
			"repository",
			"repositoryIdentity",
		]) {
			const response = await core.callDaemonRpc(
				handle.socketPath,
				handshake({
					id: `forged-${forgedField}`,
					method: "GET /v1/memories/:id",
					body: {
						id: ids.private,
						requestId: `forged-${forgedField}`,
						[forgedField]: forgedField === "modelLocal" ? true : "local",
					},
				}),
			);
			expect(response, forgedField).toMatchObject({ error: { code: "unknown_field" } });
		}
	});
});
