import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { DaemonJobService } from "./daemon-jobs.js";
import { type DaemonRpcContext, dispatchDaemonRpc } from "./daemon-rpc.js";
import { LOCAL_API_VERSION, RPC_CAPABILITY_HASH } from "./daemon-rpc-contract.js";
import { connect } from "./db.js";
import { NORMALIZED_SCHEMA_VERSION } from "./normalized-event.js";
import { SecretScanner } from "./secret-scanner.js";
import { resolveStorageLayout, sha256File } from "./storage.js";
import { MemoryStore } from "./store.js";
import { initTestSchema } from "./test-utils.js";
import type { WriterActor } from "./writer-actor.js";

describe("daemon jobs", () => {
	let db: WriterActor | null = null;
	let store: MemoryStore | null = null;
	let dir: string | null = null;

	afterEach(() => {
		store?.close();
		store = null;
		if (db?.open) db.close();
		db = null;
		if (dir) rmSync(dir, { recursive: true, force: true });
		dir = null;
	});

	it("P1-T045-02-job-no-auto-retry", async () => {
		dir = mkdtempSync(join(tmpdir(), "codemem-daemon-jobs-"));
		db = connect(join(dir, "jobs.sqlite"));
		initTestSchema(db);
		const submittedAt = "2026-08-14T00:00:00.000Z";
		db.exec(`
			INSERT INTO sessions(started_at) VALUES ('2026-08-14T00:00:00.000Z');
			INSERT INTO memory_items(
				session_id, kind, title, body_text, active, created_at, updated_at,
				metadata_json, files_read, scope_id
			) VALUES (
				last_insert_rowid(), 'discovery', 'Legacy memory', 'Needs backfill', 1,
				'2026-08-14T00:00:00.000Z', '2026-08-14T00:00:00.000Z', '{}',
				'["README.md"]', NULL
			);
		`);
		db.prepare(
			`INSERT INTO daemon_jobs(
				job_id, kind, args_json, dry_run, state, attempts, max_attempts,
				result_json, error_code, submitted_at, started_at, finished_at
			) VALUES (?, 'scopes.backfill', '{"internal":true}', 0, 'queued', 0, 1, NULL, NULL, ?, NULL, NULL)`,
		).run("00000000-0000-4000-8000-000000000001", submittedAt);
		db.prepare(
			`INSERT INTO daemon_jobs(
				job_id, kind, args_json, dry_run, state, attempts, max_attempts,
				result_json, error_code, submitted_at, started_at, finished_at
			) VALUES (?, 'refs.backfill', '{}', 0, 'running', 1, 1, NULL, NULL, ?, ?, NULL)`,
		).run("00000000-0000-4000-8000-000000000002", submittedAt, "2026-08-14T00:00:01.000Z");

		store = new MemoryStore(db);
		const service = new DaemonJobService(store);

		expect(service.get("00000000-0000-4000-8000-000000000001")).toMatchObject({
			state: "failed",
			attempts: 0,
			error: { code: "daemon_restarted" },
		});
		expect(service.get("00000000-0000-4000-8000-000000000002")).toMatchObject({
			state: "failed",
			attempts: 1,
			error: { code: "daemon_restarted" },
		});
		expect(service.get("00000000-0000-4000-8000-000000000099")).toBeNull();
		expect(() => service.get("not-a-job-id")).toThrow("job id is invalid");
		expect(() => service.submit({ kind: "scopes.backfill", args: { internal: true } })).toThrow(
			"Unknown job argument: internal",
		);
		service.startInternalBackfills();
		await service.stop();
		expect(service.list({ kind: "scopes.backfill" })).toHaveLength(2);
		expect(service.list({ kind: "refs.backfill" })).toHaveLength(1);
		expect(() => service.list({ kind: "unknown" })).toThrow("job kind is unsupported");
		expect(() => service.list({ state: "cancelled" })).toThrow("job state is invalid");
		expect(() => service.list({ submittedAfter: "yesterday" })).toThrow(
			"submittedAfter must be an ISO timestamp",
		);

		expect(() => service.submit({ kind: "db.init" })).toThrow("service is stopping");
		expect(() => service.schedule(() => {})).toThrow("service is stopping");
	});

	it("rejects structured maintenance while the frozen provider is pending", () => {
		dir = mkdtempSync(join(tmpdir(), "codemem-daemon-jobs-pending-provider-"));
		db = connect(join(dir, "jobs.sqlite"));
		initTestSchema(db);
		store = new MemoryStore(db);
		const service = new DaemonJobService(store, {
			capability: {
				providerEnabled: false,
				runtimeReason: "pending_privacy_boundary",
			},
		});

		expect(() => service.submit({ kind: "structured.backfill", args: {} })).toThrow(
			/pending_privacy_boundary/i,
		);
		expect(service.list({ kind: "structured.backfill" })).toEqual([]);
	});

	it.each([
		[
			"configured",
			{
				providerEnabled: false,
				runtimeReason: "pending_privacy_boundary",
				embeddingProvider: { state: "disabled" },
			},
		],
		["capture-only", { providerEnabled: false, runtimeReason: "manifest_absent" }],
		["missing", undefined],
	] as const)("keeps vectors untouched when %s capability disables semantics", async (_mode, capability) => {
		dir = mkdtempSync(join(tmpdir(), "codemem-daemon-semantic-disabled-"));
		db = connect(join(dir, "jobs.sqlite"));
		initTestSchema(db);
		const sessionId = Number(
			db.prepare("INSERT INTO sessions(started_at) VALUES ('2026-08-31T00:00:00Z')").run()
				.lastInsertRowid,
		);
		const insertMemory = db.prepare(
			`INSERT INTO memory_items(
				session_id, kind, title, body_text, active, created_at, updated_at,
				metadata_json, files_read, scope_id
			 ) VALUES (?, 'discovery', ?, ?, 1, '2026-08-31T00:00:00Z',
				'2026-08-31T00:00:00Z', '{}', '[]', NULL)`,
		);
		const preservedMemoryId = Number(
			insertMemory.run(sessionId, "Preserved vector", "must not be deleted").lastInsertRowid,
		);
		insertMemory.run(sessionId, "Missing vector", "must not be embedded");
		db.prepare(
			`INSERT INTO memory_vectors(embedding, memory_id, chunk_index, content_hash, model)
			 VALUES (vec_f32(?), ${preservedMemoryId}, 0, 'preserved', 'legacy-model')`,
		).run(JSON.stringify(Array(384).fill(0)));
		const before = db
			.prepare("SELECT memory_id, content_hash, model FROM memory_vectors ORDER BY memory_id")
			.all();
		store = new MemoryStore(db);
		const service = new DaemonJobService(store, { dataDir: dir, capability });

		let admissionError: unknown;
		try {
			service.submit({ kind: "vectors.migrate", args: {} });
		} catch (error) {
			admissionError = error;
		}
		service.startInternalBackfills();
		await service.stop();

		expect(admissionError).toEqual(
			expect.objectContaining({ message: expect.stringContaining("semantic_disabled") }),
		);
		expect(service.list({ kind: "vectors.migrate" })).toEqual([]);
		expect(
			db
				.prepare("SELECT memory_id, content_hash, model FROM memory_vectors ORDER BY memory_id")
				.all(),
		).toEqual(before);
	});

	it("rechecks semantic capability before executing a queued vector job", async () => {
		dir = mkdtempSync(join(tmpdir(), "codemem-daemon-semantic-recheck-"));
		db = connect(join(dir, "jobs.sqlite"));
		initTestSchema(db);
		store = new MemoryStore(db);
		const sessionId = store.startSession({ project: "semantic-recheck" });
		const memoryId = store.remember(sessionId, "discovery", "Preserved", "vector");
		db.prepare(
			`INSERT INTO memory_vectors(embedding, memory_id, chunk_index, content_hash, model)
			 VALUES (vec_f32(?), ${memoryId}, 0, 'preserved', 'legacy-model')`,
		).run(JSON.stringify(Array(384).fill(0)));
		const before = db.prepare("SELECT memory_id, content_hash, model FROM memory_vectors").all();
		const capability = {
			providerEnabled: false,
			runtimeReason: "pending_privacy_boundary",
			embeddingProvider: { state: "enabled" },
		};
		const service = new DaemonJobService(store, { dataDir: dir, capability });
		const submitted = service.submit({ kind: "vectors.migrate", args: {} });
		capability.embeddingProvider.state = "disabled";

		await service.stop();

		expect(service.get(submitted.jobId)).toMatchObject({
			state: "failed",
			attempts: 1,
			error: { code: "job_failed" },
		});
		expect(db.prepare("SELECT memory_id, content_hash, model FROM memory_vectors").all()).toEqual(
			before,
		);
	});

	it("P1-T046-01-maintenance-mode", async () => {
		dir = mkdtempSync(join(tmpdir(), "codemem-daemon-maintenance-"));
		db = connect(join(dir, "jobs.sqlite"));
		initTestSchema(db);
		store = new MemoryStore(db);
		let enterMaintenance = () => {};
		const entered = new Promise<void>((resolve) => {
			enterMaintenance = resolve;
		});
		let releaseMaintenance = () => {};
		const released = new Promise<void>((resolve) => {
			releaseMaintenance = resolve;
		});
		const service = new DaemonJobService(store, {
			dataDir: dir,
			beforeMaintenance: async () => {
				enterMaintenance();
				await released;
			},
		});
		const rpcContext = {
			identity: { pid: process.pid, startTime: "test", fingerprint: "test", nonce: "test" },
			dataDir: dir,
			onStop: () => {},
			writer: db,
			store,
			viewerAuth: {} as never,
			viewerRead: async () => ({}),
			jobs: service,
		} as DaemonRpcContext;
		const doctorRetryRequest = JSON.stringify({
			id: "maintenance-doctor-retry",
			method: "POST /v1/processing-jobs/:id/doctor-retry",
			adapter_version: "test",
			native_cli_version: "test",
			normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
			local_api_version: LOCAL_API_VERSION,
			capability_hash: RPC_CAPABILITY_HASH,
			body: {
				id: 1,
				producerReceiptId: "maintenance-doctor-retry",
				expectedRole: "summary",
				expectedProviderFingerprint: `sha256:${"a".repeat(64)}`,
				expectedManifestFingerprint: `sha256:${"b".repeat(64)}`,
				expectedAttemptCount: 3,
				expectedClaimGeneration: 3,
			},
		});
		const invalidRequests: Array<{
			input: { kind: unknown; args?: unknown; dryRun?: unknown };
			error: string;
		}> = [
			{ input: { kind: false }, error: "job kind is unsupported" },
			{ input: { kind: "db.init", args: [] }, error: "args must be an object" },
			{
				input: { kind: "db.init", args: { surprise: true } },
				error: "Unknown job argument: surprise",
			},
			{
				input: { kind: "report.db-size", args: { limit: 0 } },
				error: "args.limit must be an integer between 1 and 10000",
			},
			{
				input: { kind: "report.db-size", args: { limit: 1.5 } },
				error: "args.limit must be an integer between 1 and 10000",
			},
			{
				input: { kind: "gate.raw-events", args: { minFlushSuccessRate: Number.NaN } },
				error: "args.minFlushSuccessRate must be between 0 and 1",
			},
			{
				input: { kind: "gate.raw-events", args: { maxDroppedEventRate: 2 } },
				error: "args.maxDroppedEventRate must be between 0 and 1",
			},
			{
				input: { kind: "report.artifact", args: { includeInactive: "yes" } },
				error: "args.includeInactive must be a boolean",
			},
			{
				input: { kind: "report.artifact", args: { project: "bad\0project" } },
				error: "args.project is invalid",
			},
			{
				input: { kind: "report.memory-role", args: { probes: "probe" } },
				error: "args.probes is invalid",
			},
			{
				input: { kind: "report.memory-role", args: { probes: [""] } },
				error: "args.probes is invalid",
			},
			{
				input: { kind: "report.memory-role", args: { probes: Array(51).fill("probe") } },
				error: "args.probes is invalid",
			},
			{
				input: { kind: "tags.backfill", args: { since: "yesterday" } },
				error: "args.since must be an ISO timestamp",
			},
			{
				input: { kind: "report.role-compare", args: {} },
				error: "args.baselineDbPath is required",
			},
			{
				input: { kind: "report.role-compare", args: { baselineDbPath: "/tmp/baseline" } },
				error: "args.candidateDbPath is required",
			},
			{
				input: { kind: "projects.rename", args: { oldName: "old" } },
				error: "projects.rename requires oldName and newName",
			},
			{
				input: { kind: "raw-events.prune", args: {} },
				error: "raw-events.prune requires maxAgeDays",
			},
			{
				input: { kind: "report.extraction", args: { scenarioId: "scenario" } },
				error: "report.extraction requires scenarioId and exactly one of sessionId or batchId",
			},
			{
				input: {
					kind: "report.extraction",
					args: { scenarioId: "scenario", sessionId: 1, batchId: 1 },
				},
				error: "report.extraction requires scenarioId and exactly one of sessionId or batchId",
			},
			{ input: { kind: "db.init", dryRun: "yes" }, error: "dryRun must be a boolean" },
		];
		for (const request of invalidRequests) {
			expect(() => service.submit(request.input)).toThrow(request.error);
		}
		expect(() => service.submit({ kind: "raw-events.retry", args: {}, dryRun: true })).toThrow(
			"raw-events.retry does not support dryRun",
		);
		const waitForTerminal = async (jobId: string) => {
			for (let attempt = 0; attempt < 100; attempt++) {
				const job = service.get(jobId);
				if (job?.state === "completed" || job?.state === "failed") return job;
				await new Promise((resolve) => setTimeout(resolve, 10));
			}
			return service.get(jobId);
		};

		const submitted = service.submit({ kind: "projects.normalize", args: {}, dryRun: false });
		expect(service.hasPendingWork()).toBe(true);
		await entered;
		expect(service.isMaintenanceMode()).toBe(true);
		const response = await dispatchDaemonRpc(
			JSON.stringify({
				id: "maintenance-write",
				method: "POST /v1/memories/record",
				adapter_version: "test",
				native_cli_version: "test",
				normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
				local_api_version: LOCAL_API_VERSION,
				capability_hash: RPC_CAPABILITY_HASH,
				body: {
					idempotencyKey: "maintenance-write",
					kind: "decision",
					title: "must spool",
					body: "do not write during maintenance",
				},
			}),
			rpcContext,
		);
		expect(response).toMatchObject({
			error: { code: "maintenance_mode", retryable: true },
		});
		expect(await dispatchDaemonRpc(doctorRetryRequest, rpcContext)).toMatchObject({
			error: { code: "maintenance_mode", retryable: true },
		});

		releaseMaintenance();
		const completed = await waitForTerminal(submitted.jobId);
		expect(completed).toMatchObject({ state: "completed", attempts: 1 });
		expect(service.hasPendingWork()).toBe(false);
		expect(service.isMaintenanceMode()).toBe(false);
		expect(
			await dispatchDaemonRpc(doctorRetryRequest, {
				...rpcContext,
				restoreState: { active: true },
			}),
		).toMatchObject({ error: { code: "maintenance_mode", retryable: true } });
		const backupDir = resolveStorageLayout(dir).backupsDir;
		expect(readdirSync(backupDir).sort()).toEqual([
			`maintenance-${submitted.jobId}.json`,
			`maintenance-${submitted.jobId}.sqlite`,
		]);
		const backupPath = join(backupDir, `maintenance-${submitted.jobId}.sqlite`);
		const backupHash = sha256File(backupPath);
		expect(
			JSON.parse(readFileSync(join(backupDir, `maintenance-${submitted.jobId}.json`), "utf8")),
		).toMatchObject({ manifest: { retention_class: "manual" } });
		const compared = service.submit({
			kind: "report.role-compare",
			args: { baselineDbPath: backupPath, candidateDbPath: backupPath },
			dryRun: true,
		});
		expect(await waitForTerminal(compared.jobId)).toMatchObject({
			state: "completed",
			attempts: 1,
		});
		expect(sha256File(backupPath)).toBe(backupHash);

		db.exec(`
			INSERT INTO sessions(started_at, cwd, project, user, tool_version) VALUES
				('2026-08-14T00:00:00.000Z', '/tmp/literal', 'root/a%b_c', 'test', 'test'),
				('2026-08-14T00:00:01.000Z', '/tmp/zeta', 'root/zeta', 'test', 'test');
			INSERT INTO raw_event_sessions(
				source, stream_id, opencode_session_id, cwd, project, started_at,
				last_seen_ts_wall_ms, last_received_event_seq, last_flushed_event_seq, updated_at
			) VALUES
				('opencode', 'rename', 'rename', '/tmp/literal', 'root/a%b_c',
				 '2026-08-14T00:00:00.000Z', 1, 0, 0, '2026-08-14T00:00:00.000Z'),
				('opencode', 'normalize', 'normalize', '/tmp/alpha', 'root/alpha',
				 '2026-08-14T00:00:01.000Z', 1, 0, 0, '2026-08-14T00:00:01.000Z');
		`);
		const renameDryRun = service.submit({
			kind: "projects.rename",
			args: { oldName: "a%b_c", newName: "literal" },
			dryRun: true,
		});
		expect(await waitForTerminal(renameDryRun.jobId)).toMatchObject({
			state: "completed",
			result: { counts: { sessions: 1, raw_event_sessions: 1 } },
		});
		expect(
			(
				db.prepare("SELECT COUNT(*) AS count FROM sessions WHERE project = 'root/a%b_c'").get() as {
					count: number;
				}
			).count,
		).toBe(1);
		const renamed = service.submit({
			kind: "projects.rename",
			args: { oldName: "a%b_c", newName: "literal" },
			dryRun: false,
		});
		expect(await waitForTerminal(renamed.jobId)).toMatchObject({
			state: "completed",
			result: {
				counts: { sessions: 1, raw_event_sessions: 1 },
				backupId: `maintenance-${renamed.jobId}`,
			},
		});
		expect(
			(
				db.prepare("SELECT COUNT(*) AS count FROM sessions WHERE project = 'literal'").get() as {
					count: number;
				}
			).count,
		).toBe(1);

		const normalizeDryRun = service.submit({
			kind: "projects.normalize",
			args: {},
			dryRun: true,
		});
		expect(await waitForTerminal(normalizeDryRun.jobId)).toMatchObject({
			state: "completed",
			result: {
				counts: { sessions: 1, raw_event_sessions: 1 },
				rewrites: [
					{ from: "root/alpha", to: "alpha" },
					{ from: "root/zeta", to: "zeta" },
				],
			},
		});
		const normalized = service.submit({ kind: "projects.normalize", args: {}, dryRun: false });
		expect(await waitForTerminal(normalized.jobId)).toMatchObject({
			state: "completed",
			result: {
				counts: { sessions: 1, raw_event_sessions: 1 },
				backupId: `maintenance-${normalized.jobId}`,
			},
		});
		expect(
			db
				.prepare(
					"SELECT project FROM sessions WHERE project IN ('literal', 'zeta') ORDER BY project",
				)
				.all(),
		).toEqual([{ project: "literal" }, { project: "zeta" }]);

		for (const request of [
			{ kind: "db.init", args: {} },
			{ kind: "report.db-size", args: { limit: 1 } },
			{ kind: "report.raw-events", args: { limit: 1 } },
			{ kind: "raw-events.prune", args: { maxAgeDays: 1, vacuum: false }, dryRun: true },
			{ kind: "raw-events.prune", args: { maxAgeDays: 1, vacuum: true }, dryRun: false },
		]) {
			const job = service.submit(request);
			expect(await waitForTerminal(job.jobId)).toMatchObject({ state: "completed", attempts: 1 });
		}
		db.prepare("INSERT INTO sessions(started_at, project) VALUES (?, ?)").run(
			"2026-08-14T00:00:00.000Z",
			"team/demo",
		);
		rmSync(backupDir, { recursive: true, force: true });
		writeFileSync(backupDir, "backup directory blocked", { mode: 0o600 });
		const blocked = service.submit({ kind: "projects.normalize", args: {}, dryRun: false });
		expect(await waitForTerminal(blocked.jobId)).toMatchObject({ state: "failed", attempts: 1 });
		expect(
			(
				db.prepare("SELECT project FROM sessions ORDER BY id DESC LIMIT 1").get() as {
					project: string;
				}
			).project,
		).toBe("team/demo");
		await service.stop();
	});

	it("bounds configured regexes used by daemon maintenance jobs", async () => {
		dir = mkdtempSync(join(tmpdir(), "codemem-daemon-redaction-worker-"));
		db = connect(join(dir, "jobs.sqlite"));
		initTestSchema(db);
		store = new MemoryStore(db);
		const sessionId = store.startSession({ project: "redaction-worker" });
		store.remember(sessionId, "discovery", "seed", `${"a".repeat(26)}!`);
		store.scanner = new SecretScanner({
			rules: [{ kind: "catastrophic", pattern: /(a+)+$/g }],
		});
		const service = new DaemonJobService(store);
		const submitted = service.submit({ kind: "secrets.scan", args: { limit: 1 }, dryRun: true });
		let result = service.get(submitted.jobId);
		for (let attempt = 0; attempt < 100 && result?.state !== "failed"; attempt++) {
			await new Promise((resolve) => setTimeout(resolve, 10));
			result = service.get(submitted.jobId);
		}
		expect(result).toMatchObject({
			state: "failed",
			attempts: 1,
			error: { code: "redaction_degraded" },
		});
		await service.stop();
	});
});
