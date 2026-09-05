import { createHash } from "node:crypto";
import {
	lstatSync,
	readdirSync,
	readFileSync,
	realpathSync,
	renameSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import type { DaemonJobService } from "./daemon-jobs.js";
import { exportMemoriesWithDb, importMemoriesWithDb, readImportPayload } from "./export-import.js";
import { hashMutationPayload } from "./mutation-dispatcher.js";
import { expandUserPath } from "./observer-config.js";
import {
	BackupRequestError,
	backupPayloadHash,
	createCanonicalBackup,
	createOnlineBackup,
	recoverCanonicalBackupResult,
	recoverCanonicalRestoreResult,
	requireVerifiedBackup,
	restoreCanonicalBackup,
	restorePayloadHash,
	verifyOnlineBackup,
} from "./online-backup.js";
import { resolveStorageLayout, sha256File } from "./storage.js";
import { durableReplaceFile, ensurePrivateDirectory, fsyncPath } from "./storage-platform.js";
import type { MemoryStore } from "./store.js";

type ExportFilters = {
	project?: string;
	allProjects?: boolean;
	includeInactive?: boolean;
	since?: string;
};

type ExportRequest = { outputPath: string; filters: ExportFilters };
type ImportRequest = { inputPath: string; remapProject?: string; dryRun?: boolean };
type BackupCreateRequest = { reason: string };
type BackupRestoreRequest = { backupId: string };
type OperationRequest = ExportRequest | ImportRequest | BackupCreateRequest | BackupRestoreRequest;
type OperationKind = "export" | "import" | "backup-create" | "backup-restore";
type OperationState =
	| "prepared"
	| "writing"
	| "verified"
	| "backup_verified"
	| "applying"
	| "committed"
	| "failed";

type OperationJournal = {
	version: 1;
	operationId: string;
	payloadHash: string;
	kind: OperationKind;
	state: OperationState;
	request: OperationRequest;
	result: Record<string, unknown> | null;
	error: { code: string; message: string } | null;
	createdAt: string;
	updatedAt: string;
};

export type DaemonOperationSnapshot = Pick<
	OperationJournal,
	"operationId" | "payloadHash" | "state" | "result" | "error"
>;

export class DaemonOperationRequestError extends Error {
	constructor(
		readonly code:
			| "invalid_request"
			| "idempotency_conflict"
			| "not_found"
			| "conflict"
			| "internal_error",
		message: string,
	) {
		super(message);
		this.name = "DaemonOperationRequestError";
	}
}

const OPERATION_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[a-f0-9]{64}$/;
const TERMINAL_STATES = new Set<OperationState>(["committed", "failed"]);
const FILTER_FIELDS = new Set(["project", "allProjects", "includeInactive", "since"]);

function boundedString(value: unknown, field: string, maxBytes = 4_096): string {
	if (
		typeof value !== "string" ||
		value.length === 0 ||
		value.includes("\0") ||
		Buffer.byteLength(value, "utf8") > maxBytes
	) {
		throw new DaemonOperationRequestError("invalid_request", `${field} is invalid.`);
	}
	return value;
}

function exportRequest(body: Record<string, unknown>): ExportRequest {
	const outputPath = boundedString(body.outputPath, "outputPath");
	if (outputPath === "-") {
		throw new DaemonOperationRequestError("invalid_request", "outputPath must be a file path.");
	}
	if (!body.filters || typeof body.filters !== "object" || Array.isArray(body.filters)) {
		throw new DaemonOperationRequestError("invalid_request", "filters must be an object.");
	}
	const source = body.filters as Record<string, unknown>;
	const unknown = Object.keys(source).find((field) => !FILTER_FIELDS.has(field));
	if (unknown) {
		throw new DaemonOperationRequestError("invalid_request", `Unknown export filter: ${unknown}`);
	}
	const filters: ExportFilters = {};
	if (source.project !== undefined)
		filters.project = boundedString(source.project, "filters.project", 512);
	for (const field of ["allProjects", "includeInactive"] as const) {
		if (source[field] === undefined) continue;
		if (typeof source[field] !== "boolean") {
			throw new DaemonOperationRequestError(
				"invalid_request",
				`filters.${field} must be a boolean.`,
			);
		}
		filters[field] = source[field];
	}
	if (source.since !== undefined) {
		const since = boundedString(source.since, "filters.since", 64);
		if (!Number.isFinite(Date.parse(since))) {
			throw new DaemonOperationRequestError(
				"invalid_request",
				"filters.since must be an ISO date.",
			);
		}
		filters.since = since;
	}
	if (filters.allProjects && filters.project) {
		throw new DaemonOperationRequestError(
			"invalid_request",
			"filters.project cannot be combined with allProjects.",
		);
	}
	return { outputPath, filters };
}

function importRequest(body: Record<string, unknown>): ImportRequest {
	const inputPath = boundedString(body.inputPath, "inputPath");
	if (inputPath === "-") {
		throw new DaemonOperationRequestError("invalid_request", "inputPath must be a file path.");
	}
	const request: ImportRequest = { inputPath };
	if (body.remapProject !== undefined) {
		request.remapProject = boundedString(body.remapProject, "remapProject", 4_096);
	}
	if (body.dryRun !== undefined) {
		if (typeof body.dryRun !== "boolean") {
			throw new DaemonOperationRequestError("invalid_request", "dryRun must be a boolean.");
		}
		request.dryRun = body.dryRun;
	}
	return request;
}

function backupCreateRequest(body: Record<string, unknown>): BackupCreateRequest {
	return { reason: boundedString(body.reason, "reason", 1_024) };
}

function backupRestoreRequest(body: Record<string, unknown>): BackupRestoreRequest {
	const backupId = boundedString(body.backupId, "backupId", 128);
	if (!OPERATION_ID.test(backupId)) {
		throw new DaemonOperationRequestError("invalid_request", "backupId is invalid.");
	}
	return { backupId };
}

function parseOperationRequest(
	kind: OperationKind,
	body: Record<string, unknown>,
): OperationRequest {
	if (kind === "export") return exportRequest(body);
	if (kind === "import") return importRequest(body);
	if (kind === "backup-create") return backupCreateRequest(body);
	return backupRestoreRequest(body);
}

function operationPayloadHash(kind: OperationKind, request: OperationRequest): string {
	if (kind === "backup-create") return backupPayloadHash((request as BackupCreateRequest).reason);
	if (kind === "backup-restore") {
		return restorePayloadHash((request as BackupRestoreRequest).backupId);
	}
	return hashMutationPayload(request);
}

function operationPath(value: string): string {
	return resolve(expandUserPath(value));
}

function exportOutputPath(value: string): string {
	const path = operationPath(value);
	const parent = dirname(path);
	const parentInfo = lstatSync(parent);
	if (parentInfo.isSymbolicLink() || !parentInfo.isDirectory()) {
		throw new DaemonOperationRequestError(
			"invalid_request",
			"Export output parent must be a directory, not a symbolic link.",
		);
	}
	return join(realpathSync(parent), basename(path));
}

function isInside(parent: string, candidate: string): boolean {
	const child = relative(parent, candidate);
	return child === "" || (!child.startsWith(`..${sep}`) && child !== "..");
}

function exportTempPath(path: string, operationId: string): string {
	return `${path}.${hashMutationPayload(operationId).slice(0, 32)}.tmp`;
}

function replaceOutputFile(path: string, operationId: string, contents: string): void {
	const parent = dirname(path);
	const parentInfo = lstatSync(parent);
	if (parentInfo.isSymbolicLink() || !parentInfo.isDirectory()) {
		throw new Error("Export output parent must be a directory, not a symbolic link.");
	}
	const temporaryPath = exportTempPath(path, operationId);
	try {
		writeFileSync(temporaryPath, contents, {
			encoding: "utf8",
			flag: "wx",
			mode: 0o600,
			flush: true,
		});
		renameSync(temporaryPath, path);
		fsyncPath(parent);
	} catch (error) {
		try {
			unlinkSync(temporaryPath);
		} catch {
			// The temporary file may not exist or may already have been renamed.
		}
		throw error;
	}
}

function removeInterruptedExportTemps(journal: OperationJournal): boolean {
	if (journal.kind !== "export") return true;
	try {
		const outputPath = exportOutputPath((journal.request as ExportRequest).outputPath);
		const parent = dirname(outputPath);
		const temporaryPath = exportTempPath(outputPath, journal.operationId);
		let info: ReturnType<typeof lstatSync>;
		try {
			info = lstatSync(temporaryPath);
		} catch (error) {
			return (error as NodeJS.ErrnoException).code === "ENOENT";
		}
		if (info.isSymbolicLink() || !info.isFile()) return false;
		unlinkSync(temporaryPath);
		fsyncPath(parent);
		return true;
	} catch {
		return false;
	}
}

function parseJournal(raw: string, fileName: string): OperationJournal {
	const parsed = JSON.parse(raw) as Partial<OperationJournal>;
	if (
		parsed.version !== 1 ||
		typeof parsed.operationId !== "string" ||
		`${parsed.operationId}.json` !== fileName ||
		!OPERATION_ID.test(parsed.operationId) ||
		typeof parsed.payloadHash !== "string" ||
		!SHA256.test(parsed.payloadHash) ||
		!(["export", "import", "backup-create", "backup-restore"] as const).includes(
			parsed.kind as OperationKind,
		) ||
		typeof parsed.state !== "string" ||
		![
			"prepared",
			"writing",
			"verified",
			"backup_verified",
			"applying",
			"committed",
			"failed",
		].includes(parsed.state) ||
		!parsed.request ||
		typeof parsed.request !== "object" ||
		Array.isArray(parsed.request) ||
		typeof parsed.createdAt !== "string" ||
		typeof parsed.updatedAt !== "string"
	) {
		throw new Error(`Daemon operation journal is malformed: ${fileName}`);
	}
	const request = parseOperationRequest(
		parsed.kind as OperationKind,
		parsed.request as Record<string, unknown>,
	);
	if (operationPayloadHash(parsed.kind as OperationKind, request) !== parsed.payloadHash) {
		throw new Error(`Daemon operation journal hash is invalid: ${fileName}`);
	}
	return { ...parsed, request } as OperationJournal;
}

export function recoverDaemonRestoresBeforeOpen(dataDir: string): void {
	const directory = join(resolveStorageLayout(dataDir).controlDir, "operations");
	ensurePrivateDirectory(directory);
	for (const fileName of readdirSync(directory)
		.filter((name) => name.endsWith(".json"))
		.sort()) {
		const path = join(directory, fileName);
		const journal = parseJournal(readFileSync(path, "utf8"), fileName);
		if (journal.kind !== "backup-restore" || journal.state === "committed") continue;
		const request = journal.request as BackupRestoreRequest;
		const recovered = recoverCanonicalRestoreResult({
			dataDir,
			operationId: journal.operationId,
			payloadHash: journal.payloadHash,
			backupId: request.backupId,
		});
		if (!recovered) continue;
		durableReplaceFile(
			path,
			`${JSON.stringify({
				...journal,
				state: "committed",
				result: recovered,
				error: null,
				updatedAt: new Date().toISOString(),
			} satisfies OperationJournal)}\n`,
		);
	}
}

export class DaemonOperationService {
	private readonly directory: string;
	private readonly journals = new Map<string, OperationJournal>();
	private readonly completions = new Map<string, Promise<void>>();

	constructor(
		private readonly store: MemoryStore,
		private readonly jobs: DaemonJobService,
		private readonly dataDir: string,
	) {
		this.directory = join(resolveStorageLayout(dataDir).controlDir, "operations");
		ensurePrivateDirectory(this.directory);
		for (const fileName of readdirSync(this.directory)
			.filter((name) => name.endsWith(".json"))
			.sort()) {
			const journal = parseJournal(readFileSync(join(this.directory, fileName), "utf8"), fileName);
			this.journals.set(journal.operationId, journal);
			const recovered = journal.state === "committed" ? null : this.recoverBackupResult(journal);
			if (recovered) {
				this.transition(journal.operationId, "committed", recovered);
				continue;
			}
			if (!TERMINAL_STATES.has(journal.state)) {
				const cleanupVerified = removeInterruptedExportTemps(journal);
				this.persist({
					...journal,
					state: "failed",
					error: {
						code: "daemon_restarted",
						message: cleanupVerified
							? "The daemon restarted before the operation completed."
							: "The daemon restarted; interrupted export cleanup could not be verified.",
					},
					updatedAt: new Date().toISOString(),
				});
			}
		}
	}

	submit(
		kind: OperationKind,
		body: Record<string, unknown>,
	): { operationId: string; state: string } {
		const operationId = boundedString(body.operationId, "operationId", 128);
		if (!OPERATION_ID.test(operationId)) {
			throw new DaemonOperationRequestError("invalid_request", "operationId is invalid.");
		}
		const payloadHash = boundedString(body.payloadHash, "payloadHash", 64);
		if (!SHA256.test(payloadHash)) {
			throw new DaemonOperationRequestError("invalid_request", "payloadHash is invalid.");
		}
		const request = parseOperationRequest(kind, body);
		if (operationPayloadHash(kind, request) !== payloadHash) {
			throw new DaemonOperationRequestError(
				"invalid_request",
				"payloadHash does not match the operation request.",
			);
		}
		if (
			kind === "export" &&
			isInside(
				realpathSync(resolve(this.dataDir)),
				exportOutputPath((request as ExportRequest).outputPath),
			)
		) {
			throw new DaemonOperationRequestError(
				"invalid_request",
				"Export output must be outside the daemon data directory.",
			);
		}

		const existing = this.journals.get(operationId);
		if (existing) {
			if (existing.kind !== kind || existing.payloadHash !== payloadHash) {
				throw new DaemonOperationRequestError(
					kind === "backup-create" || kind === "backup-restore"
						? "conflict"
						: "idempotency_conflict",
					"Operation ID already exists with a different payload.",
				);
			}
			if (
				(kind === "backup-create" || kind === "backup-restore") &&
				existing.state === "failed" &&
				existing.error?.code === "daemon_restarted"
			) {
				const recovered = this.recoverBackupResult(existing);
				if (recovered) {
					this.transition(operationId, "committed", recovered);
					return { operationId, state: "committed" };
				}
				this.persist({
					...existing,
					state: "prepared",
					result: null,
					error: null,
					updatedAt: new Date().toISOString(),
				});
				try {
					this.schedule(operationId, kind, request);
				} catch {
					this.fail(operationId, "daemon_stopping", "The daemon stopped before the operation ran.");
				}
				return { operationId, state: "prepared" };
			}
			return { operationId, state: existing.state };
		}

		const now = new Date().toISOString();
		this.persist({
			version: 1,
			operationId,
			payloadHash,
			kind,
			state: "prepared",
			request,
			result: null,
			error: null,
			createdAt: now,
			updatedAt: now,
		});
		try {
			this.schedule(operationId, kind, request);
		} catch {
			this.fail(operationId, "daemon_stopping", "The daemon stopped before the operation ran.");
		}
		return { operationId, state: "prepared" };
	}

	async runBackup(
		kind: "backup-create" | "backup-restore",
		body: Record<string, unknown>,
	): Promise<Record<string, unknown>> {
		const accepted = this.submit(kind, body);
		const completion = this.completions.get(accepted.operationId);
		if (completion) {
			try {
				await completion;
			} catch {
				// The authoritative backup/restore result is checked below.
			}
		}
		const journal = this.journals.get(accepted.operationId);
		if (journal?.state === "committed" && journal.result) return journal.result;
		if (journal) {
			try {
				const recovered = this.recoverBackupResult(journal);
				if (recovered) {
					try {
						this.transition(journal.operationId, "committed", recovered);
					} catch {
						// The online-backup or restore result remains the durable authority.
					}
					return recovered;
				}
			} catch {
				// Fall through to the sanitized operation error.
			}
		}
		if (journal?.state === "failed" && journal.error) {
			const code = ["invalid_request", "conflict", "idempotency_conflict", "not_found"].includes(
				journal.error.code,
			)
				? (journal.error.code as
						| "invalid_request"
						| "conflict"
						| "idempotency_conflict"
						| "not_found")
				: "internal_error";
			throw new DaemonOperationRequestError(code, journal.error.message);
		}
		throw new DaemonOperationRequestError(
			"internal_error",
			"The daemon backup operation did not produce a durable result.",
		);
	}

	get(operationIdValue: unknown): DaemonOperationSnapshot {
		const operationId = boundedString(operationIdValue, "id", 128);
		if (!OPERATION_ID.test(operationId)) {
			throw new DaemonOperationRequestError("invalid_request", "Operation ID is invalid.");
		}
		const journal = this.journals.get(operationId);
		if (!journal) {
			throw new DaemonOperationRequestError("not_found", "Operation was not found.");
		}
		return {
			operationId: journal.operationId,
			payloadHash: journal.payloadHash,
			state: journal.state,
			result: journal.result,
			error: journal.error,
		};
	}

	hasPending(): boolean {
		return [...this.journals.values()].some((journal) => !TERMINAL_STATES.has(journal.state));
	}

	private async execute(operationId: string): Promise<void> {
		const journal = this.journals.get(operationId);
		if (journal?.state !== "prepared") return;
		if (journal.kind === "export") this.executeExport(journal);
		else if (journal.kind === "import") await this.executeImport(journal);
		else if (journal.kind === "backup-create") await this.executeBackupCreate(journal);
		else this.executeBackupRestore(journal);
	}

	private schedule(operationId: string, kind: OperationKind, request: OperationRequest): void {
		const completion = this.jobs
			.schedule(
				async () => {
					await new Promise<void>((resolve) => setImmediate(resolve));
					await this.execute(operationId);
				},
				(kind === "import" && !(request as ImportRequest).dryRun) || kind === "backup-restore",
			)
			.catch(() => {
				this.fail(operationId, "operation_failed", "The daemon operation failed.");
			});
		this.completions.set(operationId, completion);
		void completion.finally(() => {
			if (this.completions.get(operationId) === completion) this.completions.delete(operationId);
		});
	}

	private async executeBackupCreate(journal: OperationJournal): Promise<void> {
		try {
			const request = journal.request as BackupCreateRequest;
			const proof = await createCanonicalBackup({
				dataDir: this.dataDir,
				operationId: journal.operationId,
				payloadHash: journal.payloadHash,
				reason: request.reason,
			});
			this.transition(journal.operationId, "committed", {
				operationId: proof.backupId,
				backupId: proof.backupId,
				state: "completed",
				artifactSha256: proof.artifactSha256,
				manifestHash: proof.manifestHash,
			});
		} catch (error) {
			this.recoverOrFailBackup(journal, error, "The daemon backup operation failed.");
		}
	}

	private executeBackupRestore(journal: OperationJournal): void {
		try {
			const request = journal.request as BackupRestoreRequest;
			this.transition(
				journal.operationId,
				"committed",
				restoreCanonicalBackup({
					dataDir: this.dataDir,
					operationId: journal.operationId,
					payloadHash: journal.payloadHash,
					backupId: request.backupId,
				}),
			);
		} catch (error) {
			this.recoverOrFailBackup(journal, error, "The daemon restore operation failed.");
		}
	}

	private recoverOrFailBackup(
		journal: OperationJournal,
		error: unknown,
		fallbackMessage: string,
	): void {
		try {
			const recovered = this.recoverBackupResult(journal);
			if (recovered) {
				this.transition(journal.operationId, "committed", recovered);
				return;
			}
		} catch {
			// Preserve the original operation failure below.
		}
		this.failBackup(journal.operationId, error, fallbackMessage);
	}

	private failBackup(operationId: string, error: unknown, fallbackMessage: string): void {
		if (error instanceof BackupRequestError) {
			this.fail(operationId, error.code, error.message);
			return;
		}
		this.fail(operationId, "operation_failed", fallbackMessage);
	}

	private recoverBackupResult(journal: OperationJournal): Record<string, unknown> | null {
		if (journal.kind === "backup-create") {
			const proof = recoverCanonicalBackupResult({
				dataDir: this.dataDir,
				operationId: journal.operationId,
				payloadHash: journal.payloadHash,
			});
			return proof
				? {
						operationId: proof.backupId,
						backupId: proof.backupId,
						state: "completed",
						artifactSha256: proof.artifactSha256,
						manifestHash: proof.manifestHash,
					}
				: null;
		}
		if (journal.kind !== "backup-restore") return null;
		const request = journal.request as BackupRestoreRequest;
		return recoverCanonicalRestoreResult({
			dataDir: this.dataDir,
			operationId: journal.operationId,
			payloadHash: journal.payloadHash,
			backupId: request.backupId,
		});
	}

	private executeExport(journal: OperationJournal): void {
		try {
			const request = journal.request as ExportRequest;
			this.transition(journal.operationId, "writing");
			const payload = exportMemoriesWithDb(this.store.db, request.filters);
			const text = `${JSON.stringify(payload, null, 2)}\n`;
			const outputPath = exportOutputPath(request.outputPath);
			if (isInside(realpathSync(resolve(this.dataDir)), outputPath)) {
				throw new Error("Export output resolves inside the daemon data directory.");
			}
			replaceOutputFile(outputPath, journal.operationId, text);
			const expectedHash = createHash("sha256").update(text, "utf8").digest("hex");
			const outputSha256 = sha256File(outputPath);
			if (outputSha256 !== expectedHash) throw new Error("Export verification failed.");
			const result = {
				outputPath,
				outputSha256,
				sessions: payload.sessions.length,
				memory_items: payload.memory_items.length,
				session_summaries: payload.session_summaries.length,
				user_prompts: payload.user_prompts.length,
			};
			this.transition(journal.operationId, "verified", result);
			this.transition(journal.operationId, "committed", result);
		} catch {
			this.fail(journal.operationId, "export_failed", "The daemon export operation failed.");
		}
	}

	private async executeImport(journal: OperationJournal): Promise<void> {
		const request = journal.request as ImportRequest;
		let failureCode = "invalid_import";
		try {
			const payload = readImportPayload(operationPath(request.inputPath));
			const preview = importMemoriesWithDb(this.store.db, payload, {
				remapProject: request.remapProject,
				dryRun: true,
			});
			if (request.dryRun) {
				this.transition(journal.operationId, "applying");
				this.transition(journal.operationId, "committed", { ...preview });
				return;
			}

			failureCode = "backup_failed";
			const backupId = `import-${hashMutationPayload(journal.operationId).slice(0, 32)}`;
			const proof = await createOnlineBackup({
				db: this.store.db,
				destinationDir: resolveStorageLayout(this.dataDir).backupsDir,
				operationId: backupId,
				reason: `Before daemon import ${journal.operationId}`,
				retentionClass: "manual",
			});
			requireVerifiedBackup(proof);
			const check = verifyOnlineBackup({
				artifactPath: proof.artifactPath,
				expectedSha256: proof.artifactSha256,
				sourcePath: this.store.db.name,
			});
			if (!check.valid) throw new Error("Import backup verification failed.");
			this.transition(journal.operationId, "backup_verified", { backupId });

			failureCode = "import_failed";
			this.transition(journal.operationId, "applying", { backupId });
			const result = importMemoriesWithDb(this.store.db, payload, {
				remapProject: request.remapProject,
			});
			this.transition(journal.operationId, "committed", { ...result, backupId });
		} catch {
			const message =
				failureCode === "backup_failed"
					? "The verified import backup could not be created."
					: failureCode === "invalid_import"
						? "The import input is invalid or unavailable."
						: "The daemon import operation failed.";
			this.fail(journal.operationId, failureCode, message);
		}
	}

	private transition(
		operationId: string,
		state: OperationState,
		result?: Record<string, unknown>,
	): void {
		const journal = this.journals.get(operationId);
		if (!journal) return;
		this.persist({
			...journal,
			state,
			result: result ?? journal.result,
			error: null,
			updatedAt: new Date().toISOString(),
		});
	}

	private fail(operationId: string, code: string, message: string): void {
		const journal = this.journals.get(operationId);
		if (!journal || TERMINAL_STATES.has(journal.state)) return;
		this.persist({
			...journal,
			state: "failed",
			error: { code, message },
			updatedAt: new Date().toISOString(),
		});
	}

	private persist(journal: OperationJournal): void {
		durableReplaceFile(
			join(this.directory, `${journal.operationId}.json`),
			`${JSON.stringify(journal)}\n`,
		);
		this.journals.set(journal.operationId, journal);
	}
}
