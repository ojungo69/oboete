import { existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DaemonJobService } from "./daemon-jobs.js";
import { DaemonOperationService } from "./daemon-operations.js";
import { connect } from "./db.js";
import * as core from "./index.js";
import { MemoryStore } from "./store.js";
import { ReadOnlyActor } from "./writer-actor.js";

const handles: Array<{ stop: () => Promise<void> }> = [];
const roots: string[] = [];

function tempDataDir(): string {
	const root = mkdtempSync(join(tmpdir(), "codemem-operations-"));
	roots.push(root);
	return join(root, "data");
}

function handshake(method: core.RpcMethod, body: Record<string, unknown>): core.RpcRequest {
	return {
		id: crypto.randomUUID(),
		method,
		adapter_version: "test",
		native_cli_version: "test",
		normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
		local_api_version: core.LOCAL_API_VERSION,
		capability_hash: core.RPC_CAPABILITY_HASH,
		body,
	};
}

async function request(
	handle: { socketPath: string },
	method: core.RpcMethod,
	body: Record<string, unknown>,
): Promise<Record<string, unknown>> {
	const response = await core.callDaemonRpc(handle.socketPath, handshake(method, body));
	if ("error" in response) throw new Error(`${response.error.code}: ${response.error.message}`);
	return response.result;
}

async function waitForTerminal(
	handle: { socketPath: string },
	operationId: string,
): Promise<Record<string, unknown>> {
	const deadline = Date.now() + 10_000;
	while (Date.now() < deadline) {
		const operation = await request(handle, "GET /v1/operations/:id", { id: operationId });
		if (operation.state === "committed" || operation.state === "failed") return operation;
		await new Promise((resolve) => setTimeout(resolve, 10));
	}
	throw new Error(`operation did not finish: ${operationId}`);
}

afterEach(async () => {
	for (const handle of handles.splice(0)) await handle.stop();
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
	vi.restoreAllMocks();
});

describe("daemon class B operations", { timeout: 20_000 }, () => {
	it("P1-T047-01-operation-id-conflict", async () => {
		const pendingDataDir = tempDataDir();
		const pendingDb = connect(join(dirname(pendingDataDir), "pending.sqlite"));
		core.initTestSchema(pendingDb);
		const pendingStore = new MemoryStore(pendingDb, { closeConnection: true });
		let scheduled = 0;
		const pendingJobs = {
			isMaintenanceMode: () => false,
			hasPendingWork: () => false,
			schedule: () => {
				scheduled++;
				return new Promise<void>(() => {});
			},
		} as unknown as DaemonJobService;
		try {
			const pendingOperations = new DaemonOperationService(
				pendingStore,
				pendingJobs,
				pendingDataDir,
			);
			const validationOutputPath = join(dirname(pendingDataDir), "validation-export.json");
			const invalidRequests: Array<{
				kind: "export" | "import";
				body: Record<string, unknown>;
				error: string;
			}> = [
				{
					kind: "export",
					body: {
						operationId: "bad/id",
						payloadHash: "0".repeat(64),
						outputPath: validationOutputPath,
						filters: {},
					},
					error: "operationId is invalid",
				},
				{
					kind: "export",
					body: {
						operationId: "invalid-hash",
						payloadHash: "not-a-hash",
						outputPath: validationOutputPath,
						filters: {},
					},
					error: "payloadHash is invalid",
				},
				{
					kind: "export",
					body: {
						operationId: "missing-filters",
						payloadHash: "0".repeat(64),
						outputPath: validationOutputPath,
					},
					error: "filters must be an object",
				},
				{
					kind: "export",
					body: {
						operationId: "unknown-filter",
						payloadHash: "0".repeat(64),
						outputPath: validationOutputPath,
						filters: { unknown: true },
					},
					error: "Unknown export filter: unknown",
				},
				{
					kind: "export",
					body: {
						operationId: "invalid-boolean",
						payloadHash: "0".repeat(64),
						outputPath: validationOutputPath,
						filters: { includeInactive: "yes" },
					},
					error: "filters.includeInactive must be a boolean",
				},
				{
					kind: "export",
					body: {
						operationId: "invalid-since",
						payloadHash: "0".repeat(64),
						outputPath: validationOutputPath,
						filters: { since: "yesterday" },
					},
					error: "filters.since must be an ISO date",
				},
				{
					kind: "export",
					body: {
						operationId: "conflicting-filters",
						payloadHash: "0".repeat(64),
						outputPath: validationOutputPath,
						filters: { project: "demo", allProjects: true },
					},
					error: "filters.project cannot be combined with allProjects",
				},
				{
					kind: "export",
					body: {
						operationId: "stdout-export",
						payloadHash: "0".repeat(64),
						outputPath: "-",
						filters: {},
					},
					error: "outputPath must be a file path",
				},
				{
					kind: "import",
					body: { operationId: "stdin-import", payloadHash: "0".repeat(64), inputPath: "-" },
					error: "inputPath must be a file path",
				},
				{
					kind: "import",
					body: {
						operationId: "invalid-remap",
						payloadHash: "0".repeat(64),
						inputPath: validationOutputPath,
						remapProject: "bad\0project",
					},
					error: "remapProject is invalid",
				},
				{
					kind: "import",
					body: {
						operationId: "invalid-dry-run",
						payloadHash: "0".repeat(64),
						inputPath: validationOutputPath,
						dryRun: "yes",
					},
					error: "dryRun must be a boolean",
				},
				{
					kind: "export",
					body: {
						operationId: "hash-mismatch",
						payloadHash: "0".repeat(64),
						outputPath: validationOutputPath,
						filters: { allProjects: true },
					},
					error: "payloadHash does not match the operation request",
				},
			];
			for (const invalid of invalidRequests) {
				expect(() => pendingOperations.submit(invalid.kind, invalid.body)).toThrow(invalid.error);
			}
			expect(() => pendingOperations.get("bad/id")).toThrow("Operation ID is invalid");
			expect(() => pendingOperations.get("missing-operation")).toThrow("Operation was not found");
			const insideBody = {
				outputPath: join(pendingDataDir, "inside-data-dir.json"),
				filters: { allProjects: true },
			};
			expect(() =>
				pendingOperations.submit("export", {
					operationId: "inside-data-dir",
					payloadHash: core.hashMutationPayload(insideBody),
					...insideBody,
				}),
			).toThrow("outside the daemon data directory");
			const pendingBody = {
				outputPath: join(dirname(pendingDataDir), "pending-export.json"),
				filters: { allProjects: true },
			};
			pendingOperations.submit("export", {
				operationId: "pending-export",
				payloadHash: core.hashMutationPayload(pendingBody),
				...pendingBody,
			});
			expect(scheduled).toBe(1);
			expect(pendingOperations.hasPending()).toBe(true);
			const restoreConflict = await core.dispatchDaemonRpc(
				JSON.stringify(
					handshake("POST /v1/backup/restore", {
						operationId: "pending-restore",
						payloadHash: core.restorePayloadHash("missing-backup"),
						backupId: "missing-backup",
					}),
				),
				{
					identity: { pid: process.pid, nonce: "pending" },
					dataDir: pendingDataDir,
					onStop: () => {},
					writer: pendingDb,
					store: pendingStore,
					viewerAuth: {} as never,
					viewerRead: async () => ({}),
					jobs: pendingJobs,
					operations: pendingOperations,
				} as Parameters<typeof core.dispatchDaemonRpc>[1],
			);
			expect(restoreConflict).toMatchObject({ error: { code: "conflict" } });
		} finally {
			pendingStore.close();
		}

		const stoppingDataDir = tempDataDir();
		const stoppingDb = connect(join(dirname(stoppingDataDir), "stopping.sqlite"));
		core.initTestSchema(stoppingDb);
		const stoppingStore = new MemoryStore(stoppingDb, { closeConnection: true });
		const stoppingJobs = {
			schedule: () => {
				throw new Error("daemon is stopping");
			},
		} as unknown as DaemonJobService;
		try {
			const stoppingOperations = new DaemonOperationService(
				stoppingStore,
				stoppingJobs,
				stoppingDataDir,
			);
			const stoppingBody = {
				outputPath: join(dirname(stoppingDataDir), "stopping-export.json"),
				filters: { allProjects: true },
			};
			expect(
				stoppingOperations.submit("export", {
					operationId: "stopping-export",
					payloadHash: core.hashMutationPayload(stoppingBody),
					...stoppingBody,
				}),
			).toEqual({ operationId: "stopping-export", state: "prepared" });
			expect(stoppingOperations.get("stopping-export")).toMatchObject({
				state: "failed",
				error: { code: "daemon_stopping" },
			});
		} finally {
			stoppingStore.close();
		}

		const scheduledDataDir = tempDataDir();
		const scheduledDb = connect(join(dirname(scheduledDataDir), "scheduled.sqlite"));
		core.initTestSchema(scheduledDb);
		const scheduledStore = new MemoryStore(scheduledDb, { closeConnection: true });
		const scheduledJobs = new DaemonJobService(scheduledStore);
		try {
			const scheduledOperations = new DaemonOperationService(
				scheduledStore,
				scheduledJobs,
				scheduledDataDir,
			);
			const outputPath = join(dirname(scheduledDataDir), "scheduled-export.json");
			const requestBody = { outputPath, filters: { allProjects: true } };
			expect(
				scheduledOperations.submit("export", {
					operationId: "scheduled-export",
					payloadHash: core.hashMutationPayload(requestBody),
					...requestBody,
				}),
			).toEqual({ operationId: "scheduled-export", state: "prepared" });
			expect(scheduledOperations.hasPending()).toBe(true);
			await Promise.resolve();
			expect(existsSync(outputPath)).toBe(false);
			await scheduledJobs.stop();
			expect(existsSync(outputPath)).toBe(true);
		} finally {
			await scheduledJobs.stop();
			scheduledStore.close();
		}

		const dataDir = tempDataDir();
		const handle = await core.startDaemon({ dataDir });
		handles.push(handle);
		const operationId = crypto.randomUUID();
		const firstBody = {
			outputPath: join(dirname(dataDir), "first.json"),
			filters: { allProjects: true },
		};
		const secondBody = {
			outputPath: join(dirname(dataDir), "second.json"),
			filters: { allProjects: true },
		};
		await request(handle, "POST /v1/operations/export", {
			operationId,
			payloadHash: core.hashMutationPayload(firstBody),
			...firstBody,
		});
		const conflict = await core.callDaemonRpc(
			handle.socketPath,
			handshake("POST /v1/operations/export", {
				operationId,
				payloadHash: core.hashMutationPayload(secondBody),
				...secondBody,
			}),
		);
		expect(conflict).toMatchObject({ error: { code: "idempotency_conflict" } });
		expect(await waitForTerminal(handle, operationId)).toMatchObject({ state: "committed" });
		expect(existsSync(secondBody.outputPath)).toBe(false);

		const escapeLink = join(dirname(dataDir), "data-link");
		symlinkSync(dataDir, escapeLink);
		const unsafeBody = {
			outputPath: join(escapeLink, "db", "must-not-write.json"),
			filters: { allProjects: true },
		};
		expect(
			await core.callDaemonRpc(
				handle.socketPath,
				handshake("POST /v1/operations/export", {
					operationId: crypto.randomUUID(),
					payloadHash: core.hashMutationPayload(unsafeBody),
					...unsafeBody,
				}),
			),
		).toMatchObject({ error: { code: "invalid_request" } });
	});

	it("P1-T047-02-operation-result-retrieval", async () => {
		const recoveryDataDir = tempDataDir();
		const recoveryDb = connect(join(dirname(recoveryDataDir), "recovery.sqlite"));
		core.initTestSchema(recoveryDb);
		const recoveryStore = new MemoryStore(recoveryDb, { closeConnection: true });
		const recoveryJobs = {
			schedule: () => Promise.resolve(),
		} as unknown as DaemonJobService;
		try {
			new DaemonOperationService(recoveryStore, recoveryJobs, recoveryDataDir);
			const operationsDir = join(
				core.resolveStorageLayout(recoveryDataDir).controlDir,
				"operations",
			);
			const interrupted = [
				{
					operationId: "recover-clean-export",
					outputPath: join(dirname(recoveryDataDir), "recover-clean.json"),
					cleanupVerified: true,
				},
				{
					operationId: "recover-symlink-export",
					outputPath: join(dirname(recoveryDataDir), "recover-symlink.json"),
					cleanupVerified: false,
				},
			];
			for (const candidate of interrupted) {
				const requestBody = {
					outputPath: candidate.outputPath,
					filters: { allProjects: true },
				};
				const temporaryPath = `${candidate.outputPath}.${core
					.hashMutationPayload(candidate.operationId)
					.slice(0, 32)}.tmp`;
				if (candidate.cleanupVerified) {
					writeFileSync(temporaryPath, "interrupted export", { mode: 0o600 });
				} else {
					const target = join(dirname(recoveryDataDir), "symlink-target");
					writeFileSync(target, "must not be removed", { mode: 0o600 });
					symlinkSync(target, temporaryPath);
				}
				const now = "2026-08-14T00:00:00.000Z";
				writeFileSync(
					join(operationsDir, `${candidate.operationId}.json`),
					`${JSON.stringify({
						version: 1,
						operationId: candidate.operationId,
						payloadHash: core.hashMutationPayload(requestBody),
						kind: "export",
						state: "writing",
						request: requestBody,
						result: null,
						error: null,
						createdAt: now,
						updatedAt: now,
					})}\n`,
					{ mode: 0o600 },
				);
			}
			const recovered = new DaemonOperationService(recoveryStore, recoveryJobs, recoveryDataDir);
			expect(recovered.get("recover-clean-export")).toMatchObject({
				state: "failed",
				error: {
					code: "daemon_restarted",
					message: "The daemon restarted before the operation completed.",
				},
			});
			expect(recovered.get("recover-symlink-export")).toMatchObject({
				state: "failed",
				error: {
					code: "daemon_restarted",
					message: "The daemon restarted; interrupted export cleanup could not be verified.",
				},
			});
			expect(
				existsSync(
					`${interrupted[0].outputPath}.${core
						.hashMutationPayload(interrupted[0].operationId)
						.slice(0, 32)}.tmp`,
				),
			).toBe(false);
			expect(
				existsSync(
					`${interrupted[1].outputPath}.${core
						.hashMutationPayload(interrupted[1].operationId)
						.slice(0, 32)}.tmp`,
				),
			).toBe(true);
			expect(recovered.hasPending()).toBe(false);
		} finally {
			recoveryStore.close();
		}

		const dataDir = tempDataDir();
		const outputPath = join(dirname(dataDir), "export.json");
		const operationId = crypto.randomUUID();
		const operationBody = { outputPath, filters: { allProjects: true } };
		const first = await core.startDaemon({ dataDir });
		handles.push(first);
		await request(first, "POST /v1/operations/export", {
			operationId,
			payloadHash: core.hashMutationPayload(operationBody),
			...operationBody,
		});
		const completed = await waitForTerminal(first, operationId);
		expect(completed).toMatchObject({
			operationId,
			payloadHash: core.hashMutationPayload(operationBody),
			state: "committed",
			result: { outputSha256: expect.stringMatching(/^[a-f0-9]{64}$/) },
		});
		expect(JSON.parse(readFileSync(outputPath, "utf8"))).toMatchObject({ version: "2.0" });
		expect(
			await request(first, "POST /v1/operations/export", {
				operationId,
				payloadHash: core.hashMutationPayload(operationBody),
				...operationBody,
			}),
		).toEqual({ operationId, state: "committed" });

		await first.stop();
		const restarted = await core.startDaemon({ dataDir });
		handles.push(restarted);
		expect(await request(restarted, "GET /v1/operations/:id", { id: operationId })).toEqual(
			completed,
		);

		const delayedDataDir = tempDataDir();
		const delayed = await core.startDaemon({ dataDir: delayedDataDir });
		handles.push(delayed);
		const delayedOperationId = "lost-backup-response";
		const delayedReason = "response loss durability";
		let backupStarted: (() => void) | undefined;
		const backupStart = new Promise<void>((resolve) => {
			backupStarted = resolve;
		});
		let releaseBackup: (() => void) | undefined;
		const backupGate = new Promise<void>((resolve) => {
			releaseBackup = resolve;
		});
		const originalBackup = ReadOnlyActor.prototype.backup;
		const backupSpy = vi
			.spyOn(ReadOnlyActor.prototype, "backup")
			.mockImplementationOnce(async function (destinationFile, options) {
				backupStarted?.();
				await backupGate;
				return originalBackup.call(this, destinationFile, options);
			});
		const delayedBody = {
			operationId: delayedOperationId,
			reason: delayedReason,
			payloadHash: core.backupPayloadHash(delayedReason),
		};
		const lostResponse = core
			.callDaemonRpc(delayed.socketPath, handshake("POST /v1/backup/create", delayedBody), {
				timeoutMs: 25,
			})
			.then(
				() => null,
				(error: unknown) => error,
			);
		await backupStart;
		expect(await lostResponse).toBeInstanceOf(Error);
		releaseBackup?.();
		const delayedCompleted = await waitForTerminal(delayed, delayedOperationId);
		expect(delayedCompleted).toMatchObject({
			operationId: delayedOperationId,
			payloadHash: core.backupPayloadHash(delayedReason),
			state: "committed",
			result: {
				operationId: delayedOperationId,
				backupId: delayedOperationId,
				state: "completed",
				artifactSha256: expect.stringMatching(/^[a-f0-9]{64}$/),
				manifestHash: expect.stringMatching(/^[a-f0-9]{64}$/),
			},
		});
		expect(await request(delayed, "POST /v1/backup/create", delayedBody)).toEqual(
			delayedCompleted.result,
		);
		expect(
			await core.callDaemonRpc(
				delayed.socketPath,
				handshake("POST /v1/backup/create", {
					...delayedBody,
					reason: "different response loss payload",
					payloadHash: core.backupPayloadHash("different response loss payload"),
				}),
			),
		).toMatchObject({ error: { code: "conflict" } });
		expect(backupSpy).toHaveBeenCalledTimes(1);
		backupSpy.mockRestore();
		await delayed.stop();
		const delayedJournalPath = join(
			core.resolveStorageLayout(delayedDataDir).controlDir,
			"operations",
			`${delayedOperationId}.json`,
		);
		const delayedJournal = JSON.parse(readFileSync(delayedJournalPath, "utf8")) as Record<
			string,
			unknown
		>;
		writeFileSync(
			delayedJournalPath,
			`${JSON.stringify({
				...delayedJournal,
				state: "failed",
				result: null,
				error: { code: "operation_failed", message: "response was lost" },
			})}\n`,
			{ mode: 0o600 },
		);
		const recoveredBackup = await core.startDaemon({ dataDir: delayedDataDir });
		handles.push(recoveredBackup);
		expect(
			await request(recoveredBackup, "GET /v1/operations/:id", { id: delayedOperationId }),
		).toEqual(delayedCompleted);

		const crashDataDir = tempDataDir();
		const crashed = await core.startDaemon({ dataDir: crashDataDir });
		handles.push(crashed);
		await crashed.stop();
		const crashLayout = core.resolveStorageLayout(crashDataDir);
		const crashOperationId = "artifact-before-complete";
		const crashReason = "replay interrupted backup";
		const crashPayloadHash = core.backupPayloadHash(crashReason);
		const nonRetryOperationId = "failed-for-another-reason";
		const nonRetryReason = "do not replay other failures";
		const now = "2026-08-14T00:00:00.000Z";
		writeFileSync(join(crashLayout.backupsDir, `${crashOperationId}.tmp`), "interrupted", {
			mode: 0o600,
		});
		for (const failed of [
			{
				operationId: crashOperationId,
				reason: crashReason,
				payloadHash: crashPayloadHash,
				code: "daemon_restarted",
			},
			{
				operationId: nonRetryOperationId,
				reason: nonRetryReason,
				payloadHash: core.backupPayloadHash(nonRetryReason),
				code: "operation_failed",
			},
		]) {
			writeFileSync(
				join(crashLayout.controlDir, "operations", `${failed.operationId}.json`),
				`${JSON.stringify({
					version: 1,
					operationId: failed.operationId,
					payloadHash: failed.payloadHash,
					kind: "backup-create",
					state: "failed",
					request: { reason: failed.reason },
					result: null,
					error: { code: failed.code, message: "The backup was interrupted." },
					createdAt: now,
					updatedAt: now,
				})}\n`,
				{ mode: 0o600 },
			);
		}
		const replayed = await core.startDaemon({ dataDir: crashDataDir });
		handles.push(replayed);
		expect(
			await request(replayed, "GET /v1/operations/:id", { id: crashOperationId }),
		).toMatchObject({ state: "failed", error: { code: "daemon_restarted" } });
		const replaySpy = vi.spyOn(ReadOnlyActor.prototype, "backup");
		const nonRetryBody = {
			operationId: nonRetryOperationId,
			reason: nonRetryReason,
			payloadHash: core.backupPayloadHash(nonRetryReason),
		};
		expect(
			await core.callDaemonRpc(
				replayed.socketPath,
				handshake("POST /v1/backup/create", nonRetryBody),
			),
		).toMatchObject({ error: { code: "internal_error" } });
		const crashBody = {
			operationId: crashOperationId,
			reason: crashReason,
			payloadHash: crashPayloadHash,
		};
		expect(
			await core.callDaemonRpc(
				replayed.socketPath,
				handshake("POST /v1/backup/create", {
					...crashBody,
					reason: "conflicting replay",
					payloadHash: core.backupPayloadHash("conflicting replay"),
				}),
			),
		).toMatchObject({ error: { code: "conflict" } });
		expect(replaySpy).not.toHaveBeenCalled();
		const replayResult = await request(replayed, "POST /v1/backup/create", crashBody);
		expect(replayResult).toMatchObject({
			operationId: crashOperationId,
			backupId: crashOperationId,
			state: "completed",
		});
		expect(await request(replayed, "POST /v1/backup/create", crashBody)).toEqual(replayResult);
		expect(replaySpy).toHaveBeenCalledTimes(1);
		expect(existsSync(join(crashLayout.backupsDir, `${crashOperationId}.tmp`))).toBe(false);
		expect(existsSync(join(crashLayout.backupsDir, `${crashOperationId}.sqlite`))).toBe(true);
		expect(existsSync(join(crashLayout.backupsDir, `${crashOperationId}.json`))).toBe(true);
	});

	it("P1-T047-03-import-backup-precondition", async () => {
		const dataDir = tempDataDir();
		const inputPath = join(dirname(dataDir), "import.json");
		const invalidInputPath = join(dirname(dataDir), "invalid-import.json");
		writeFileSync(
			inputPath,
			JSON.stringify({
				version: "1.0",
				exported_at: "2026-08-14T00:00:00.000Z",
				export_metadata: {
					tool_version: "codemem",
					projects: ["imported"],
					total_memories: 1,
					total_sessions: 1,
					include_inactive: false,
					filters: {},
				},
				sessions: [
					{
						id: 1,
						started_at: "2026-08-14T00:00:00.000Z",
						cwd: "/tmp/imported",
						project: "imported",
						user: "test",
						tool_version: "test",
						metadata_json: {},
						import_key: "session-1",
					},
				],
				memory_items: [
					{
						id: 1,
						session_id: 1,
						kind: "discovery",
						title: "must not import",
						body_text: "backup failed",
						created_at: "2026-08-14T00:00:01.000Z",
						updated_at: "2026-08-14T00:00:01.000Z",
						metadata_json: {},
						import_key: "memory-1",
						scope_id: "local-default",
					},
				],
				session_summaries: [],
				user_prompts: [],
			}),
			{ mode: 0o600 },
		);
		writeFileSync(invalidInputPath, "{\n", { mode: 0o600 });
		const handle = await core.startDaemon({ dataDir });
		handles.push(handle);

		const invalidImportBody = { inputPath: invalidInputPath };
		const invalidImportId = crypto.randomUUID();
		await request(handle, "POST /v1/operations/import", {
			operationId: invalidImportId,
			payloadHash: core.hashMutationPayload(invalidImportBody),
			...invalidImportBody,
		});
		expect(await waitForTerminal(handle, invalidImportId)).toMatchObject({
			state: "failed",
			error: { code: "invalid_import" },
		});

		const previewBody = { inputPath, remapProject: "preview-project", dryRun: true };
		const previewId = crypto.randomUUID();
		await request(handle, "POST /v1/operations/import", {
			operationId: previewId,
			payloadHash: core.hashMutationPayload(previewBody),
			...previewBody,
		});
		expect(await waitForTerminal(handle, previewId)).toMatchObject({
			state: "committed",
			result: { dryRun: true, memory_items: 1 },
		});
		expect(await request(handle, "GET /v1/view", { collection: "stats" })).toMatchObject({
			body: { database: { memory_items: 0 } },
		});

		rmSync(handle.layout.backupsDir, { recursive: true });
		writeFileSync(handle.layout.backupsDir, "backup directory blocked", { mode: 0o600 });

		const operationId = crypto.randomUUID();
		const operationBody = { inputPath };
		await request(handle, "POST /v1/operations/import", {
			operationId,
			payloadHash: core.hashMutationPayload(operationBody),
			...operationBody,
		});
		expect(await waitForTerminal(handle, operationId)).toMatchObject({
			state: "failed",
			error: { code: "backup_failed" },
		});
		const stats = await request(handle, "GET /v1/view", { collection: "stats" });
		expect(stats).toMatchObject({ body: { database: { memory_items: 0 } } });

		rmSync(handle.layout.backupsDir);
		const retryId = crypto.randomUUID();
		await request(handle, "POST /v1/operations/import", {
			operationId: retryId,
			payloadHash: core.hashMutationPayload(operationBody),
			...operationBody,
		});
		const completed = await waitForTerminal(handle, retryId);
		expect(completed).toMatchObject({
			state: "committed",
			result: { memory_items: 1, backupId: expect.any(String) },
		});
		const backupId = String((completed.result as Record<string, unknown>).backupId);
		expect(
			JSON.parse(readFileSync(join(handle.layout.backupsDir, `${backupId}.json`), "utf8")),
		).toMatchObject({ manifest: { retention_class: "manual" } });
		// The imported legacy row is normalized to sensitivity='secret', so the
		// eligible-only viewer stats do not count it; the committed result above
		// is the evidence that the import landed.
		expect(await request(handle, "GET /v1/view", { collection: "stats" })).toMatchObject({
			body: { database: { memory_items: 0 } },
		});

		const exportFailureDataDir = tempDataDir();
		const exportFailureDb = connect(join(dirname(exportFailureDataDir), "export-failure.sqlite"));
		core.initTestSchema(exportFailureDb);
		const exportFailureStore = new MemoryStore(exportFailureDb, { closeConnection: true });
		let scheduledWork: (() => Promise<void> | void) | null = null;
		const capturedJobs = {
			schedule: (work: () => Promise<void> | void) => {
				scheduledWork = work;
				return Promise.resolve();
			},
		} as unknown as DaemonJobService;
		try {
			const exportFailureOperations = new DaemonOperationService(
				exportFailureStore,
				capturedJobs,
				exportFailureDataDir,
			);
			const outputDir = mkdtempSync(join(dirname(exportFailureDataDir), "vanishing-export-"));
			const outputPath = join(outputDir, "export.json");
			const exportBody = { outputPath, filters: { allProjects: true } };
			exportFailureOperations.submit("export", {
				operationId: "vanishing-export",
				payloadHash: core.hashMutationPayload(exportBody),
				...exportBody,
			});
			rmSync(outputDir, { recursive: true });
			if (!scheduledWork) throw new Error("export work was not scheduled");
			await scheduledWork();
			expect(exportFailureOperations.get("vanishing-export")).toMatchObject({
				state: "failed",
				error: { code: "export_failed" },
			});
			expect(existsSync(outputPath)).toBe(false);
		} finally {
			exportFailureStore.close();
		}
	});
});
