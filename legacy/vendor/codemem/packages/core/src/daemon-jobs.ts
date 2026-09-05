import { randomUUID } from "node:crypto";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	type ProviderChoiceV1,
	providerTransportProfile,
	type ResourceProfileV1,
} from "./capability-manifest.js";
import { connect, connectReadOnly, getSchemaVersion, isEmbeddingDisabled } from "./db.js";
import {
	DEDUP_KEY_BACKFILL_JOB,
	hasPendingDedupKeyBackfill,
	runDedupKeyBackfillPass,
} from "./dedup-key-backfill.js";
import type { DestinationBoundaryV1 } from "./destination-boundary.js";
import { getSessionExtractionEval } from "./extraction-eval.js";
import {
	aiBackfillStructuredContent,
	backfillMemoryDedupKeys,
	backfillNarrativeFromBody,
	backfillTagsText,
	compareMemoryRoleReports,
	deactivateLowSignalMemories,
	deactivateLowSignalObservations,
	dedupNearDuplicateMemories,
	getMemoryArtifactReportWithDb,
	getMemoryRoleReportWithStore,
	getRawEventRelinkPlanWithDb,
	getRawEventRelinkReportFromDb,
	getRawEventStatusWithDb,
	rawEventsGateWithDb,
	retryRawEventFailuresWithDb,
	scanSecretsRetroactive,
	vacuumDatabaseWithDb,
} from "./maintenance.js";
import { getMaintenanceJob } from "./maintenance-jobs.js";
import { createOnlineBackup, requireVerifiedBackup, verifyOnlineBackup } from "./online-backup.js";
import { RedactionWorkerError, WorkerSecretScanner } from "./redaction-worker.js";
import { hasPendingRefBackfill, REF_BACKFILL_JOB, runRefBackfillPass } from "./ref-backfill.js";
import {
	hasPendingScopeBackfill,
	runScopeBackfillPass,
	SCOPE_BACKFILL_JOB,
} from "./scope-backfill.js";
import {
	hasPendingSessionContextBackfill,
	runSessionContextBackfillPass,
	SESSION_CONTEXT_BACKFILL_JOB,
} from "./session-context-backfill.js";
import { resolveStorageLayout } from "./storage.js";
import { MemoryStore } from "./store.js";
import {
	hasPendingSummaryDedupBackfill,
	runSummaryDedupBackfillPass,
	SUMMARY_DEDUP_BACKFILL_JOB,
} from "./summary-dedup-backfill.js";
import { runVectorMigrationPass, VECTOR_MODEL_MIGRATION_JOB } from "./vector-migration.js";

const JOB_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const JOB_STATES = new Set(["queued", "running", "completed", "failed"]);
const JOB_KINDS = new Set([
	"db.init",
	"db.vacuum",
	"dedup-keys.backfill",
	"gate.raw-events",
	"memories.dedup",
	"memories.prune",
	"narrative.backfill",
	"observations.prune",
	"plan.relink",
	"projects.normalize",
	"projects.rename",
	"raw-events.prune",
	"raw-events.retry",
	"refs.backfill",
	"report.artifact",
	"report.db-size",
	"report.extraction",
	"report.memory-role",
	"report.raw-events",
	"report.relink",
	"report.role-compare",
	"scopes.backfill",
	"secrets.scan",
	"session-context.backfill",
	"structured.backfill",
	"summary-dedup.backfill",
	"tags.backfill",
	"vectors.migrate",
]);
const JOB_ARGS: Record<string, readonly string[]> = {
	"db.init": [],
	"db.vacuum": [],
	"dedup-keys.backfill": ["limit", "batchSize"],
	"gate.raw-events": [
		"minFlushSuccessRate",
		"maxDroppedEventRate",
		"minSessionBoundaryAccuracy",
		"windowHours",
	],
	"narrative.backfill": ["limit"],
	"memories.dedup": ["windowMs", "limit"],
	"memories.prune": ["limit", "kinds"],
	"observations.prune": ["limit"],
	"plan.relink": ["project", "allProjects", "limit"],
	"projects.normalize": [],
	"projects.rename": ["oldName", "newName"],
	"raw-events.prune": ["maxAgeDays", "vacuum"],
	"raw-events.retry": ["limit"],
	"refs.backfill": ["batchSize"],
	"report.artifact": ["project", "allProjects", "includeInactive"],
	"report.db-size": ["limit"],
	"report.extraction": ["sessionId", "batchId", "scenarioId", "includeInactive"],
	"report.memory-role": ["project", "allProjects", "includeInactive", "probes"],
	"report.raw-events": ["limit"],
	"report.relink": ["project", "allProjects", "limit"],
	"report.role-compare": [
		"baselineDbPath",
		"candidateDbPath",
		"project",
		"allProjects",
		"includeInactive",
		"probes",
	],
	"scopes.backfill": ["batchSize"],
	"secrets.scan": ["limit"],
	"session-context.backfill": ["batchSize"],
	"structured.backfill": ["limit", "kinds", "overwrite"],
	"summary-dedup.backfill": ["batchSize"],
	"tags.backfill": ["limit", "since", "project", "activeOnly"],
	"vectors.migrate": ["batchSize"],
};
const MAINTENANCE_JOB_KINDS = new Set([
	"db.vacuum",
	"dedup-keys.backfill",
	"memories.dedup",
	"memories.prune",
	"narrative.backfill",
	"observations.prune",
	"projects.normalize",
	"projects.rename",
	"raw-events.prune",
	"raw-events.retry",
	"refs.backfill",
	"scopes.backfill",
	"secrets.scan",
	"session-context.backfill",
	"structured.backfill",
	"summary-dedup.backfill",
	"tags.backfill",
	"vectors.migrate",
]);
const BACKUP_REQUIRED_JOB_KINDS = new Set([
	"db.vacuum",
	"memories.dedup",
	"memories.prune",
	"observations.prune",
	"projects.normalize",
	"projects.rename",
	"raw-events.prune",
	"secrets.scan",
]);
const DRY_RUN_JOB_KINDS = new Set([
	"dedup-keys.backfill",
	"memories.dedup",
	"memories.prune",
	"narrative.backfill",
	"observations.prune",
	"projects.normalize",
	"projects.rename",
	"raw-events.prune",
	"secrets.scan",
	"structured.backfill",
	"tags.backfill",
]);
const MAX_RESULT_BYTES = 512 * 1024;
const INTERNAL_SCAN_MS = 5_000;

export type DaemonJobState = "queued" | "running" | "completed" | "failed";

export type DaemonJobSnapshot = {
	jobId: string;
	kind: string;
	args: Record<string, unknown>;
	dryRun: boolean;
	state: DaemonJobState;
	attempts: number;
	maxAttempts: 1;
	result: unknown;
	error: { code: string; message: string } | null;
	submittedAt: string;
	startedAt: string | null;
	finishedAt: string | null;
};

type DaemonJobRow = {
	job_id: string;
	kind: string;
	args_json: string;
	dry_run: number;
	state: DaemonJobState;
	attempts: number;
	max_attempts: 1;
	result_json: string | null;
	error_code: string | null;
	submitted_at: string;
	started_at: string | null;
	finished_at: string | null;
};

export class DaemonJobRequestError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "DaemonJobRequestError";
	}
}

export type DaemonJobServiceOptions = {
	dataDir?: string;
	capability?: {
		providerEnabled: boolean;
		runtimeReason: string;
		embeddingProvider?: { readonly state: string };
		summaryProvider?: ProviderChoiceV1;
		resourceProfile?: ResourceProfileV1;
		destinationBoundary?: DestinationBoundaryV1;
	};
	beforeMaintenance?: () => Promise<void>;
	afterMaintenance?: () => Promise<void> | void;
};

function parseObject(value: unknown, field: string): Record<string, unknown> {
	if (value === undefined) return {};
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new DaemonJobRequestError(`${field} must be an object.`);
	}
	return value as Record<string, unknown>;
}

function optionalInteger(
	args: Record<string, unknown>,
	field: string,
	max = 10_000,
): number | null {
	const value = args[field];
	if (value === undefined || value === null) return null;
	if (!Number.isInteger(value) || Number(value) < 1 || Number(value) > max) {
		throw new DaemonJobRequestError(`args.${field} must be an integer between 1 and ${max}.`);
	}
	return Number(value);
}

function optionalNumber(
	args: Record<string, unknown>,
	field: string,
	min: number,
	max: number,
): number | null {
	const value = args[field];
	if (value === undefined || value === null) return null;
	if (typeof value !== "number" || !Number.isFinite(value) || value < min || value > max) {
		throw new DaemonJobRequestError(`args.${field} must be between ${min} and ${max}.`);
	}
	return value;
}

function optionalBoolean(args: Record<string, unknown>, field: string): boolean | null {
	const value = args[field];
	if (value === undefined || value === null) return null;
	if (typeof value !== "boolean") {
		throw new DaemonJobRequestError(`args.${field} must be a boolean.`);
	}
	return value;
}

function optionalString(
	args: Record<string, unknown>,
	field: string,
	maxBytes: number,
): string | null {
	const value = args[field];
	if (value === undefined || value === null || value === "") return null;
	if (
		typeof value !== "string" ||
		Buffer.byteLength(value, "utf8") > maxBytes ||
		value.includes("\0")
	) {
		throw new DaemonJobRequestError(`args.${field} is invalid.`);
	}
	return value;
}

function optionalStrings(
	args: Record<string, unknown>,
	field: string,
	maxItems: number,
	maxBytes: number,
): string[] {
	const value = args[field];
	if (value === undefined || value === null) return [];
	if (
		!Array.isArray(value) ||
		value.length > maxItems ||
		value.some(
			(item) =>
				typeof item !== "string" ||
				item.length === 0 ||
				Buffer.byteLength(item, "utf8") > maxBytes ||
				item.includes("\0"),
		)
	) {
		throw new DaemonJobRequestError(`args.${field} is invalid.`);
	}
	return value as string[];
}

function validateJobArgs(kind: string, args: Record<string, unknown>): void {
	const allowed = JOB_ARGS[kind] ?? [];
	const unknown = Object.keys(args).filter((field) => !allowed.includes(field));
	if (unknown.length > 0) throw new DaemonJobRequestError(`Unknown job argument: ${unknown[0]}`);
	for (const field of ["limit", "batchSize", "sessionId", "batchId"] as const) {
		optionalInteger(args, field, field.endsWith("Id") ? Number.MAX_SAFE_INTEGER : 10_000);
	}
	optionalInteger(args, "maxAgeDays", 36_500);
	optionalInteger(args, "windowMs", 31_536_000_000);
	for (const field of [
		"allProjects",
		"includeInactive",
		"activeOnly",
		"overwrite",
		"vacuum",
	] as const) {
		optionalBoolean(args, field);
	}
	for (const field of ["project", "scenarioId", "oldName", "newName"] as const) {
		optionalString(args, field, 512);
	}
	for (const field of ["baselineDbPath", "candidateDbPath"] as const) {
		optionalString(args, field, 4_096);
	}
	optionalStrings(args, "probes", 50, 4_096);
	optionalStrings(args, "kinds", 50, 128);
	if (args.since !== undefined) {
		const since = optionalString(args, "since", 64);
		if (!since || !Number.isFinite(Date.parse(since))) {
			throw new DaemonJobRequestError("args.since must be an ISO timestamp.");
		}
	}
	for (const field of [
		"minFlushSuccessRate",
		"maxDroppedEventRate",
		"minSessionBoundaryAccuracy",
	] as const) {
		optionalNumber(args, field, 0, 1);
	}
	optionalNumber(args, "windowHours", 0.01, 8_760);
	if (kind === "report.role-compare") {
		if (!optionalString(args, "baselineDbPath", 4_096)) {
			throw new DaemonJobRequestError("args.baselineDbPath is required.");
		}
		if (!optionalString(args, "candidateDbPath", 4_096)) {
			throw new DaemonJobRequestError("args.candidateDbPath is required.");
		}
	}
	if (kind === "projects.rename") {
		if (!optionalString(args, "oldName", 512) || !optionalString(args, "newName", 512)) {
			throw new DaemonJobRequestError("projects.rename requires oldName and newName.");
		}
	}
	if (kind === "raw-events.prune" && !optionalInteger(args, "maxAgeDays", 36_500)) {
		throw new DaemonJobRequestError("raw-events.prune requires maxAgeDays.");
	}
	if (kind === "report.extraction") {
		const hasSession = args.sessionId !== undefined;
		const hasBatch = args.batchId !== undefined;
		if (hasSession === hasBatch || !optionalString(args, "scenarioId", 512)) {
			throw new DaemonJobRequestError(
				"report.extraction requires scenarioId and exactly one of sessionId or batchId.",
			);
		}
	}
}

function validateJob(
	kind: unknown,
	argsValue: unknown,
): { kind: string; args: Record<string, unknown> } {
	if (typeof kind !== "string" || !JOB_KINDS.has(kind)) {
		throw new DaemonJobRequestError("job kind is unsupported.");
	}
	const args = parseObject(argsValue, "args");
	validateJobArgs(kind, args);
	return { kind, args };
}

function parseJson(value: string | null): unknown {
	if (value === null) return null;
	try {
		return JSON.parse(value) as unknown;
	} catch {
		return null;
	}
}

function escapeSqlLikePattern(value: string): string {
	return value.replaceAll("\\", String.raw`\\`).replaceAll("%", "\\%").replaceAll("_", "\\_");
}

function renameProjects(
	db: MemoryStore["db"],
	oldName: string,
	newName: string,
	dryRun: boolean,
): { counts: Record<string, number> } {
	const suffixPattern = `%/${escapeSqlLikePattern(oldName)}`;
	const tables = ["sessions", "raw_event_sessions"] as const;
	const counts: Record<string, number> = {};
	const run = () => {
		for (const table of tables) {
			const row = db
				.prepare(
					String.raw`SELECT COUNT(*) AS count FROM ${table} WHERE project = ? OR project LIKE ? ESCAPE '\'`,
				)
				.get(oldName, suffixPattern) as { count: number };
			counts[table] = row.count;
			if (dryRun || row.count === 0) continue;
			db.prepare(`UPDATE ${table} SET project = ? WHERE project = ?`).run(newName, oldName);
			db.prepare(
				String.raw`UPDATE ${table} SET project = ? WHERE project LIKE ? ESCAPE '\' AND project != ?`,
			).run(newName, suffixPattern, newName);
		}
	};
	if (dryRun) run();
	else db.transaction(run)();
	return { counts };
}

function normalizeProjects(
	db: MemoryStore["db"],
	dryRun: boolean,
): { counts: Record<string, number>; rewrites: Array<{ from: string; to: string }> } {
	const tables = ["sessions", "raw_event_sessions"] as const;
	const rewrites = new Map<string, string>();
	const counts: Record<string, number> = {};
	const run = () => {
		for (const table of tables) {
			const projects = db
				.prepare(
					`SELECT DISTINCT project FROM ${table} WHERE project IS NOT NULL AND project LIKE '%/%'`,
				)
				.all() as Array<{ project: string }>;
			let updated = 0;
			for (const row of projects) {
				const basename = row.project.split("/").pop() ?? row.project;
				if (basename === row.project) continue;
				rewrites.set(row.project, basename);
				if (dryRun) {
					const count = db
						.prepare(`SELECT COUNT(*) AS count FROM ${table} WHERE project = ?`)
						.get(row.project) as { count: number };
					updated += count.count;
				} else {
					updated += db
						.prepare(`UPDATE ${table} SET project = ? WHERE project = ?`)
						.run(basename, row.project).changes;
				}
			}
			counts[table] = updated;
		}
	};
	if (dryRun) run();
	else db.transaction(run)();
	return {
		counts,
		rewrites: [...rewrites.entries()]
			.sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
			.map(([from, to]) => ({ from, to })),
	};
}

async function getDaemonMemoryRoleReport(
	dbPath: string,
	options: Parameters<typeof getMemoryRoleReportWithStore>[1],
) {
	const directory = mkdtempSync(join(tmpdir(), "codemem-role-report-"));
	const snapshotPath = join(directory, "snapshot.sqlite");
	try {
		const source = connectReadOnly(dbPath);
		try {
			await source.backup(snapshotPath);
		} finally {
			source.close();
		}
		const store = new MemoryStore(connect(snapshotPath), { closeConnection: true });
		try {
			return getMemoryRoleReportWithStore(store, options);
		} finally {
			store.close();
		}
	} finally {
		rmSync(directory, { recursive: true, force: true });
	}
}

function snapshot(row: DaemonJobRow | undefined): DaemonJobSnapshot | null {
	if (!row) return null;
	return {
		jobId: row.job_id,
		kind: row.kind,
		args: (parseJson(row.args_json) ?? {}) as Record<string, unknown>,
		dryRun: row.dry_run === 1,
		state: row.state,
		attempts: row.attempts,
		maxAttempts: 1,
		result: parseJson(row.result_json),
		error: row.error_code
			? { code: row.error_code, message: "Daemon job failed; submit a new job to retry." }
			: null,
		submittedAt: row.submitted_at,
		startedAt: row.started_at,
		finishedAt: row.finished_at,
	};
}

export class DaemonJobService {
	private queue: Promise<void> = Promise.resolve();
	private accepting = true;
	private maintenanceMode = false;
	private scanTimer: ReturnType<typeof setInterval> | null = null;

	constructor(
		private readonly store: MemoryStore,
		private readonly options: DaemonJobServiceOptions = {},
	) {
		this.store.db
			.prepare(
				`UPDATE daemon_jobs
				 SET state = 'failed', error_code = 'daemon_restarted', finished_at = ?
				 WHERE state IN ('queued', 'running')`,
			)
			.run(new Date().toISOString());
	}

	isMaintenanceMode(): boolean {
		return this.maintenanceMode;
	}

	hasPendingWork(): boolean {
		return Boolean(
			this.store.db
				.prepare("SELECT 1 FROM daemon_jobs WHERE state IN ('queued', 'running') LIMIT 1")
				.get(),
		);
	}

	private isSemanticDisabled(): boolean {
		return this.options.capability?.embeddingProvider?.state !== "enabled" || isEmbeddingDisabled();
	}

	submit(input: { kind: unknown; args?: unknown; dryRun?: unknown }): {
		jobId: string;
		state: "queued";
	} {
		if (!this.accepting) throw new Error("Daemon job service is stopping.");
		const { kind, args } = validateJob(input.kind, input.args);
		if (kind === "vectors.migrate" && this.isSemanticDisabled()) {
			throw new DaemonJobRequestError("vectors.migrate is unavailable: semantic_disabled");
		}
		if (kind === "structured.backfill" && this.options.capability?.providerEnabled !== true) {
			throw new DaemonJobRequestError(
				`structured.backfill is unavailable: ${this.options.capability?.runtimeReason ?? "manifest_absent"}`,
			);
		}
		if (input.dryRun !== undefined && typeof input.dryRun !== "boolean") {
			throw new DaemonJobRequestError("dryRun must be a boolean.");
		}
		if (input.dryRun === true && MAINTENANCE_JOB_KINDS.has(kind) && !DRY_RUN_JOB_KINDS.has(kind)) {
			throw new DaemonJobRequestError(`${kind} does not support dryRun.`);
		}
		return this.enqueue(kind, args, input.dryRun === true);
	}

	startInternalBackfills(): void {
		if (this.scanTimer) return;
		this.scanInternalBackfills();
		this.scanTimer = setInterval(() => this.scanInternalBackfills(), INTERNAL_SCAN_MS);
		this.scanTimer.unref();
	}

	get(jobId: unknown): DaemonJobSnapshot | null {
		if (typeof jobId !== "string" || !JOB_ID.test(jobId)) {
			throw new DaemonJobRequestError("job id is invalid.");
		}
		return snapshot(
			this.store.db.prepare("SELECT * FROM daemon_jobs WHERE job_id = ? LIMIT 1").get(jobId) as
				| DaemonJobRow
				| undefined,
		);
	}

	list(
		input: { kind?: unknown; state?: unknown; submittedAfter?: unknown } = {},
	): DaemonJobSnapshot[] {
		const params: unknown[] = [];
		const where: string[] = [];
		if (input.kind !== undefined) {
			if (typeof input.kind !== "string" || !JOB_KINDS.has(input.kind)) {
				throw new DaemonJobRequestError("job kind is unsupported.");
			}
			where.push("kind = ?");
			params.push(input.kind);
		}
		if (input.state !== undefined) {
			if (typeof input.state !== "string" || !JOB_STATES.has(input.state)) {
				throw new DaemonJobRequestError("job state is invalid.");
			}
			where.push("state = ?");
			params.push(input.state);
		}
		if (input.submittedAfter !== undefined) {
			if (
				typeof input.submittedAfter !== "string" ||
				input.submittedAfter.length > 64 ||
				!Number.isFinite(Date.parse(input.submittedAfter))
			) {
				throw new DaemonJobRequestError("submittedAfter must be an ISO timestamp.");
			}
			where.push("submitted_at >= ?");
			params.push(input.submittedAfter);
		}
		const whereSuffix = where.length > 0 ? ` WHERE ${where.join(" AND ")}` : "";
		const rows = this.store.db
			.prepare(
				`SELECT * FROM daemon_jobs${whereSuffix}
				 ORDER BY submitted_at DESC, job_id DESC LIMIT 100`,
			)
			.all(...params) as DaemonJobRow[];
		return rows.map((row) => snapshot(row)).filter((row): row is DaemonJobSnapshot => row !== null);
	}

	async stop(): Promise<void> {
		this.accepting = false;
		if (this.scanTimer) clearInterval(this.scanTimer);
		this.scanTimer = null;
		await this.queue;
	}

	schedule(work: () => Promise<void> | void, maintenance = false): Promise<void> {
		if (!this.accepting) throw new Error("Daemon job service is stopping.");
		const scheduled = this.queue.then(async () => {
			if (maintenance) await this.runInMaintenance(work);
			else await work();
		});
		this.queue = scheduled.catch(() => undefined);
		return scheduled;
	}

	private enqueue(
		kind: string,
		args: Record<string, unknown>,
		dryRun: boolean,
	): {
		jobId: string;
		state: "queued";
	} {
		const jobId = randomUUID();
		this.store.db
			.prepare(
				`INSERT INTO daemon_jobs(
					job_id, kind, args_json, dry_run, state, attempts, max_attempts,
					result_json, error_code, submitted_at, started_at, finished_at
				) VALUES (?, ?, ?, ?, 'queued', 0, 1, NULL, NULL, ?, NULL, NULL)`,
			)
			.run(jobId, kind, JSON.stringify(args), dryRun ? 1 : 0, new Date().toISOString());
		this.queue = this.queue.then(() => this.run(jobId));
		return { jobId, state: "queued" };
	}

	private scanInternalBackfills(): void {
		if (!this.accepting) return;
		const plans: Array<[string, () => boolean]> = [
			["scopes.backfill", () => hasPendingScopeBackfill(this.store.db)],
			["dedup-keys.backfill", () => hasPendingDedupKeyBackfill(this.store.db)],
			["session-context.backfill", () => hasPendingSessionContextBackfill(this.store.db)],
			["refs.backfill", () => hasPendingRefBackfill(this.store.db)],
			["summary-dedup.backfill", () => hasPendingSummaryDedupBackfill(this.store.db)],
			["vectors.migrate", () => this.hasPendingVectorMigration()],
		];
		for (const [kind, pending] of plans) {
			if (!pending()) continue;
			const latest = this.store.db
				.prepare(
					`SELECT state, error_code, json_extract(args_json, '$.internal') AS internal
					 FROM daemon_jobs WHERE kind = ? ORDER BY submitted_at DESC LIMIT 1`,
				)
				.get(kind) as
				| { state: DaemonJobState; error_code: string | null; internal: number | null }
				| undefined;
			if (
				latest?.state === "queued" ||
				latest?.state === "running" ||
				(latest?.state === "failed" &&
					(latest.error_code !== "daemon_restarted" || latest.internal !== 1))
			) {
				continue;
			}
			this.enqueue(kind, { internal: true }, false);
		}
	}

	private hasPendingVectorMigration(): boolean {
		if (this.isSemanticDisabled()) return false;
		try {
			return (
				this.store.db
					.prepare(
						`SELECT 1
						 FROM memory_items m
						 WHERE m.active = 1
						   AND NOT EXISTS (SELECT 1 FROM memory_vectors v WHERE v.memory_id = m.id)
						 LIMIT 1`,
					)
					.get() !== undefined
			);
		} catch {
			return false;
		}
	}

	private async run(jobId: string): Promise<void> {
		const startedAt = new Date().toISOString();
		const claimed = this.store.db
			.prepare(
				`UPDATE daemon_jobs
				 SET state = 'running', attempts = attempts + 1, started_at = ?
				 WHERE job_id = ? AND state = 'queued' AND attempts < max_attempts`,
			)
			.run(startedAt, jobId);
		if (claimed.changes !== 1) return;
		const row = this.store.db.prepare("SELECT * FROM daemon_jobs WHERE job_id = ?").get(jobId) as
			| DaemonJobRow
			| undefined;
		if (!row) return;
		try {
			const args = parseObject(parseJson(row.args_json), "args");
			const result = await this.executeWithMaintenance(row, args);
			const resultJson = JSON.stringify(result);
			if (Buffer.byteLength(resultJson, "utf8") > MAX_RESULT_BYTES) {
				throw new Error("job result exceeds the persisted result limit");
			}
			this.store.db
				.prepare(
					`UPDATE daemon_jobs
					 SET state = 'completed', result_json = ?, error_code = NULL, finished_at = ?
					 WHERE job_id = ? AND state = 'running'`,
				)
				.run(resultJson, new Date().toISOString(), jobId);
		} catch (error) {
			this.store.db
				.prepare(
					`UPDATE daemon_jobs
					 SET state = 'failed', result_json = NULL, error_code = ?, finished_at = ?
					 WHERE job_id = ? AND state = 'running'`,
				)
				.run(
					error instanceof RedactionWorkerError ? "redaction_degraded" : "job_failed",
					new Date().toISOString(),
					jobId,
				);
		}
	}

	private async executeWithMaintenance(
		row: DaemonJobRow,
		args: Record<string, unknown>,
	): Promise<unknown> {
		const maintenance = row.dry_run === 0 && MAINTENANCE_JOB_KINDS.has(row.kind);
		if (!maintenance) return this.execute(row, args);

		return this.runInMaintenance(async () => {
			let backupId: string | null = null;
			if (BACKUP_REQUIRED_JOB_KINDS.has(row.kind) || args.internal !== true) {
				if (!this.options.dataDir)
					throw new Error("maintenance backup data directory is unavailable");
				const proof = await createOnlineBackup({
					db: this.store.db,
					destinationDir: resolveStorageLayout(this.options.dataDir).backupsDir,
					operationId: `maintenance-${row.job_id}`,
					reason: `Before daemon maintenance job ${row.kind}`,
					retentionClass: "manual",
				});
				requireVerifiedBackup(proof);
				const check = verifyOnlineBackup({
					artifactPath: proof.artifactPath,
					expectedSha256: proof.artifactSha256,
					sourcePath: this.store.db.name,
				});
				if (!check.valid) throw new Error("maintenance backup verification failed");
				backupId = proof.backupId;
			}
			const result = await this.execute(row, args);
			return backupId && result && typeof result === "object" && !Array.isArray(result)
				? { ...(result as Record<string, unknown>), backupId }
				: result;
		});
	}

	private async runInMaintenance<T>(work: () => Promise<T> | T): Promise<T> {
		this.maintenanceMode = true;
		try {
			await this.options.beforeMaintenance?.();
			return await work();
		} finally {
			try {
				await this.options.afterMaintenance?.();
			} finally {
				this.maintenanceMode = false;
			}
		}
	}

	private async runPasses(pass: () => Promise<boolean>): Promise<void> {
		// ponytail: one durable job is capped at 10k bounded passes; split work if a larger DB appears.
		for (let count = 0; count < 10_000; count++) {
			if (!(await pass())) return;
		}
		throw new Error("maintenance pass limit exceeded");
	}

	private async execute(row: DaemonJobRow, args: Record<string, unknown>): Promise<unknown> {
		const batchSize = optionalInteger(args, "batchSize") ?? undefined;
		const limit = optionalInteger(args, "limit") ?? undefined;
		const internal = args.internal === true;
		const scanner = new WorkerSecretScanner(this.store.scanner);
		switch (row.kind) {
			case "db.init":
				return {
					path: this.store.db.name,
					sizeBytes: statSync(this.store.db.name).size,
					schemaVersion: getSchemaVersion(this.store.db),
				};
			case "db.vacuum":
				return vacuumDatabaseWithDb(this.store.db);
			case "raw-events.prune": {
				const maxAgeDays = optionalInteger(args, "maxAgeDays", 36_500);
				if (!maxAgeDays) throw new Error("maxAgeDays is required");
				const maxAgeMs = maxAgeDays * 86_400_000;
				const status = this.store.rawEventsRetentionStatus(maxAgeMs);
				if (row.dry_run === 1) return { status, deleted: 0, vacuumed: null };
				const deleted = this.store.purgeRawEvents(maxAgeMs);
				const vacuumed = optionalBoolean(args, "vacuum")
					? vacuumDatabaseWithDb(this.store.db)
					: null;
				return { status, deleted, vacuumed };
			}
			case "raw-events.retry":
				return retryRawEventFailuresWithDb(this.store.db, limit ?? 25);
			case "projects.rename":
				return renameProjects(
					this.store.db,
					String(args.oldName),
					String(args.newName),
					row.dry_run === 1,
				);
			case "projects.normalize":
				return normalizeProjects(this.store.db, row.dry_run === 1);
			case "observations.prune":
				return deactivateLowSignalObservations(this.store.db, limit ?? null, row.dry_run === 1);
			case "memories.prune":
				return deactivateLowSignalMemories(this.store.db, {
					kinds: optionalStrings(args, "kinds", 50, 128),
					limit: limit ?? null,
					dryRun: row.dry_run === 1,
				});
			case "memories.dedup":
				return dedupNearDuplicateMemories(this.store.db, {
					windowMs: optionalInteger(args, "windowMs", 31_536_000_000) ?? undefined,
					limit,
					dryRun: row.dry_run === 1,
				});
			case "secrets.scan":
				return scanSecretsRetroactive(this.store.db, {
					limit,
					dryRun: row.dry_run === 1,
					scanner,
				});
			case "tags.backfill":
				return backfillTagsText(this.store.db, {
					limit,
					since: optionalString(args, "since", 64),
					project: optionalString(args, "project", 512),
					activeOnly: optionalBoolean(args, "activeOnly") ?? true,
					dryRun: row.dry_run === 1,
					scanner,
				});
			case "dedup-keys.backfill":
				if (!internal) {
					return backfillMemoryDedupKeys(this.store.db, {
						limit,
						dryRun: row.dry_run === 1,
					});
				}
				await this.runPasses(() => runDedupKeyBackfillPass(this.store.db, { batchSize }));
				return { maintenance: getMaintenanceJob(this.store.db, DEDUP_KEY_BACKFILL_JOB) };
			case "narrative.backfill":
				return backfillNarrativeFromBody(this.store.db, {
					limit,
					dryRun: row.dry_run === 1,
					scanner,
				});
			case "structured.backfill": {
				if (
					this.options.capability?.providerEnabled !== true ||
					!this.options.capability.summaryProvider ||
					!this.options.capability.resourceProfile
				) {
					throw new Error("structured.backfill has no enabled frozen provider.");
				}
				return aiBackfillStructuredContent(this.store.db, {
					limit,
					kinds: optionalStrings(args, "kinds", 50, 128),
					overwrite: optionalBoolean(args, "overwrite") ?? false,
					dryRun: row.dry_run === 1,
					scanner,
					runtimeReason: "ready",
					destinationBoundary: this.options.capability.destinationBoundary,
					summaryProvider: this.options.capability.summaryProvider,
					resourceProfile: providerTransportProfile(this.options.capability.resourceProfile),
				});
			}
			case "refs.backfill":
				await this.runPasses(() => runRefBackfillPass(this.store.db, { batchSize }));
				return { maintenance: getMaintenanceJob(this.store.db, REF_BACKFILL_JOB) };
			case "scopes.backfill":
				await this.runPasses(() => runScopeBackfillPass(this.store.db, { batchSize }));
				return { maintenance: getMaintenanceJob(this.store.db, SCOPE_BACKFILL_JOB) };
			case "session-context.backfill":
				await this.runPasses(() => runSessionContextBackfillPass(this.store.db, { batchSize }));
				return { maintenance: getMaintenanceJob(this.store.db, SESSION_CONTEXT_BACKFILL_JOB) };
			case "summary-dedup.backfill":
				await this.runPasses(() =>
					runSummaryDedupBackfillPass(this.store.db, {
						batchSize,
						deviceId: this.store.deviceId,
					}),
				);
				return { maintenance: getMaintenanceJob(this.store.db, SUMMARY_DEDUP_BACKFILL_JOB) };
			case "vectors.migrate":
				if (this.isSemanticDisabled()) {
					throw new Error("vectors.migrate is unavailable: semantic_disabled");
				}
				await this.runPasses(async () => {
					await runVectorMigrationPass(this.store.db, {
						batchSize,
						capability: this.options.capability,
					});
					const job = getMaintenanceJob(this.store.db, VECTOR_MODEL_MIGRATION_JOB);
					if (!job) {
						if (this.hasPendingVectorMigration()) {
							throw new Error("vector migration could not start");
						}
						return false;
					}
					if (job.status === "failed") throw new Error("vector migration failed");
					return job.status === "running" || job.status === "pending";
				});
				return { maintenance: getMaintenanceJob(this.store.db, VECTOR_MODEL_MIGRATION_JOB) };
			case "report.memory-role":
				return getMemoryRoleReportWithStore(this.store, {
					project: optionalString(args, "project", 512),
					allProjects: optionalBoolean(args, "allProjects") ?? false,
					includeInactive: optionalBoolean(args, "includeInactive") ?? false,
					probes: optionalStrings(args, "probes", 50, 4_096),
				});
			case "report.role-compare": {
				const options = {
					project: optionalString(args, "project", 512),
					allProjects: optionalBoolean(args, "allProjects") ?? false,
					includeInactive: optionalBoolean(args, "includeInactive") ?? false,
					probes: optionalStrings(args, "probes", 50, 4_096),
				};
				const [baseline, candidate] = await Promise.all([
					getDaemonMemoryRoleReport(String(args.baselineDbPath), options),
					getDaemonMemoryRoleReport(String(args.candidateDbPath), options),
				]);
				return compareMemoryRoleReports(baseline, candidate);
			}
			case "report.artifact":
				return getMemoryArtifactReportWithDb(this.store.db, {
					project: optionalString(args, "project", 512),
					allProjects: optionalBoolean(args, "allProjects") ?? false,
					includeInactive: optionalBoolean(args, "includeInactive") ?? false,
				});
			case "report.relink":
				return getRawEventRelinkReportFromDb(this.store.db, {
					project: optionalString(args, "project", 512),
					allProjects: optionalBoolean(args, "allProjects") ?? false,
					limit,
				});
			case "plan.relink":
				return getRawEventRelinkPlanWithDb(this.store.db, {
					project: optionalString(args, "project", 512),
					allProjects: optionalBoolean(args, "allProjects") ?? false,
					limit,
				});
			case "report.extraction":
				return args.batchId !== undefined
					? getSessionExtractionEval(this.store.db, {
							batchId: Number(args.batchId),
							scenarioId: String(args.scenarioId),
							includeInactive: optionalBoolean(args, "includeInactive") ?? false,
						})
					: getSessionExtractionEval(this.store.db, {
							sessionId: Number(args.sessionId),
							scenarioId: String(args.scenarioId),
							includeInactive: optionalBoolean(args, "includeInactive") ?? false,
						});
			case "report.raw-events":
				return getRawEventStatusWithDb(this.store.db, limit ?? 25);
			case "gate.raw-events":
				return rawEventsGateWithDb(this.store.db, {
					minFlushSuccessRate: optionalNumber(args, "minFlushSuccessRate", 0, 1) ?? undefined,
					maxDroppedEventRate: optionalNumber(args, "maxDroppedEventRate", 0, 1) ?? undefined,
					minSessionBoundaryAccuracy:
						optionalNumber(args, "minSessionBoundaryAccuracy", 0, 1) ?? undefined,
					windowHours: optionalNumber(args, "windowHours", 0.01, 8_760) ?? undefined,
				});
			case "report.db-size": {
				const pageInfo = this.store.db
					.prepare(
						"SELECT page_count * page_size AS total FROM pragma_page_count, pragma_page_size",
					)
					.get() as { total?: number } | undefined;
				const freePages = this.store.db
					.prepare("SELECT freelist_count FROM pragma_freelist_count")
					.get() as { freelist_count?: number } | undefined;
				const pageSize = this.store.db.pragma("page_size", { simple: true }) as number;
				const tables = this.store.db
					.prepare(
						`SELECT name, SUM(pgsize) AS size_bytes
						 FROM dbstat GROUP BY name ORDER BY size_bytes DESC LIMIT ?`,
					)
					.all(limit ?? 12);
				return {
					file_size_bytes: statSync(this.store.db.name).size,
					db_size_bytes: Number(pageInfo?.total ?? 0),
					free_bytes: Number(freePages?.freelist_count ?? 0) * Number(pageSize ?? 4_096),
					tables,
				};
			}
			default:
				throw new Error("unsupported daemon job");
		}
	}
}
