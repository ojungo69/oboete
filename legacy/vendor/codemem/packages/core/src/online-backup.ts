import { createHash } from "node:crypto";
import {
	chmodSync,
	existsSync,
	lstatSync,
	readdirSync,
	readFileSync,
	renameSync,
	unlinkSync,
} from "node:fs";
import { join } from "node:path";
import * as sqliteVec from "sqlite-vec";
import { MIN_WRITABLE_SCHEMA, SCHEMA_VERSION, tableExists } from "./db.js";
import {
	peekMigrationKind,
	runDatabaseMigrations,
	verifyFreshDatabase,
} from "./migration-runner.js";
import { canonicalMutationJson } from "./mutation-dispatcher.js";
import { NORMALIZED_SCHEMA_VERSION } from "./normalized-event.js";
import {
	activateDatabaseArtifact,
	readCurrentDatabasePointer,
	resolveStorageLayout,
	sha256File,
} from "./storage.js";
import {
	durableCopyFile,
	durableRemoveFile,
	durableReplaceFile,
	ensurePrivateDirectory,
	fsyncPath,
} from "./storage-platform.js";
import { ReadOnlyActor, WriterActor } from "./writer-actor.js";

export type Backupable = {
	backup(destinationFile: string): Promise<{ totalPages: number; remainingPages: number }>;
	name?: string;
};

export type BackupVerification = {
	verified: boolean;
	evidence: string;
};

export type VerifiedBackup = BackupVerification & {
	verified: true;
	backupId: string;
	artifactPath: string;
	artifactSha256: string;
	manifestHash: string;
};

export type BackupCheck = {
	valid: boolean;
	manifestHash: string | null;
	diagnostics: string[];
};

export type BackupErrorCode = "invalid_request" | "conflict" | "not_found";

export type BackupRetentionClass = "automatic" | "manual";

export type BackupManifest = {
	manifest_version: 1;
	operation_id: string;
	reason: string;
	payload_hash: string;
	artifact_sha256: string;
	schema_version: number;
	sqlite_source_version: string;
	fts_schema: {
		definitions: Array<{ type: string; name: string; sql: string }>;
		sha256: string;
		normalization_version: number;
	};
	sqlite_vec: {
		version: string;
		artifact_sha256: string;
		platform: string;
	} | null;
	active_embedding_generation_id: string | null;
	canonical_tables: Array<{
		name: string;
		row_count: number;
		checksum_sha256: string;
	}>;
	created_watermark: {
		raw_event_id: number | null;
		raw_event_created_at: string | null;
	};
	created_at: string;
	retention_class: BackupRetentionClass;
	privacy: {
		may_contain_private_or_local_only: true;
		off_device_export: "not_available_in_phase_1";
	};
};

export type BackupSidecarV2 = {
	version: 2;
	manifest: BackupManifest;
	manifest_hash: string;
	authenticity: "hash-only";
	signature: null;
};

export type BackupListEntry = {
	backupId: string;
	reason: string | null;
	createdAt: string | null;
	retentionClass: BackupRetentionClass | null;
	valid: boolean;
	manifestHash: string | null;
	artifactSha256: string | null;
	diagnostics: string[];
};

export type RestoreBackupResult = {
	operationId: string;
	backupId: string;
	pointer: string;
	artifactSha256: string;
	manifestHash: string;
	restartRequired: true;
};

type RestoreResultRecord = {
	version: 1;
	payloadHash: string;
	artifactSha256: string;
	manifestHash: string;
};

const OPERATION_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[a-f0-9]{64}$/;

export class BackupRequestError extends Error {
	readonly code: BackupErrorCode;

	constructor(code: BackupErrorCode, message: string) {
		super(message);
		this.name = "BackupRequestError";
		this.code = code;
	}
}

export function backupPayloadHash(reason: string): string {
	return createHash("sha256").update(reason, "utf8").digest("hex");
}

export function requireVerifiedBackup(proof: BackupVerification): void {
	if (!proof.verified || !proof.evidence.trim()) {
		throw new Error("Destructive operation requires a verified backup.");
	}
}

function backupArtifactPath(destinationDir: string, operationId: string): string {
	return join(destinationDir, `${operationId}.sqlite`);
}

function backupSidecarPath(destinationDir: string, operationId: string): string {
	return join(destinationDir, `${operationId}.json`);
}

function validateOperationId(operationId: string): void {
	if (!OPERATION_ID.test(operationId)) {
		throw new BackupRequestError("invalid_request", "Backup operation ID is invalid.");
	}
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isIsoDate(value: unknown): value is string {
	return typeof value === "string" && Number.isFinite(Date.parse(value));
}

function isManifest(value: unknown): value is BackupManifest {
	if (!isRecord(value)) return false;
	const fts = value.fts_schema;
	const vector = value.sqlite_vec;
	const watermark = value.created_watermark;
	const privacy = value.privacy;
	return (
		value.manifest_version === 1 &&
		typeof value.operation_id === "string" &&
		OPERATION_ID.test(value.operation_id) &&
		typeof value.reason === "string" &&
		typeof value.payload_hash === "string" &&
		SHA256.test(value.payload_hash) &&
		typeof value.artifact_sha256 === "string" &&
		SHA256.test(value.artifact_sha256) &&
		typeof value.schema_version === "number" &&
		Number.isInteger(value.schema_version) &&
		value.schema_version >= 0 &&
		typeof value.sqlite_source_version === "string" &&
		isRecord(fts) &&
		Array.isArray(fts.definitions) &&
		fts.definitions.every(
			(definition) =>
				isRecord(definition) &&
				typeof definition.type === "string" &&
				typeof definition.name === "string" &&
				typeof definition.sql === "string",
		) &&
		typeof fts.sha256 === "string" &&
		SHA256.test(fts.sha256) &&
		typeof fts.normalization_version === "number" &&
		Number.isInteger(fts.normalization_version) &&
		(vector === null ||
			(isRecord(vector) &&
				typeof vector.version === "string" &&
				typeof vector.artifact_sha256 === "string" &&
				SHA256.test(vector.artifact_sha256) &&
				typeof vector.platform === "string")) &&
		(value.active_embedding_generation_id === null ||
			typeof value.active_embedding_generation_id === "string") &&
		Array.isArray(value.canonical_tables) &&
		value.canonical_tables.every(
			(table) =>
				isRecord(table) &&
				typeof table.name === "string" &&
				typeof table.row_count === "number" &&
				Number.isInteger(table.row_count) &&
				table.row_count >= 0 &&
				typeof table.checksum_sha256 === "string" &&
				SHA256.test(table.checksum_sha256),
		) &&
		isRecord(watermark) &&
		(watermark.raw_event_id === null ||
			(typeof watermark.raw_event_id === "number" &&
				Number.isInteger(watermark.raw_event_id) &&
				watermark.raw_event_id >= 0)) &&
		(watermark.raw_event_created_at === null ||
			typeof watermark.raw_event_created_at === "string") &&
		isIsoDate(value.created_at) &&
		(value.retention_class === "automatic" || value.retention_class === "manual") &&
		isRecord(privacy) &&
		privacy.may_contain_private_or_local_only === true &&
		privacy.off_device_export === "not_available_in_phase_1"
	);
}

function readSidecar(path: string): BackupSidecarV2 | null {
	if (!existsSync(path)) return null;
	let parsed: unknown;
	try {
		const info = lstatSync(path);
		if (info.isSymbolicLink() || !info.isFile()) return null;
		parsed = JSON.parse(readFileSync(path, "utf8"));
	} catch {
		return null;
	}
	if (!isRecord(parsed)) return null;
	if (
		parsed.version !== 2 ||
		!isManifest(parsed.manifest) ||
		typeof parsed.manifest_hash !== "string" ||
		!SHA256.test(parsed.manifest_hash) ||
		parsed.authenticity !== "hash-only" ||
		parsed.signature !== null
	) {
		return null;
	}
	return parsed as BackupSidecarV2;
}

function manifestHash(manifest: BackupManifest): string {
	return createHash("sha256").update(canonicalMutationJson(manifest), "utf8").digest("hex");
}

function quoteIdentifier(value: string): string {
	return `"${value.replaceAll('"', '""')}"`;
}

function updateRowHash(hash: ReturnType<typeof createHash>, column: string, value: unknown): void {
	const name = Buffer.from(column, "utf8");
	hash.update(`c${name.length}:`).update(name);
	if (value === null) {
		hash.update("n;");
		return;
	}
	if (Buffer.isBuffer(value)) {
		hash.update(`b${value.length}:`).update(value);
		return;
	}
	const text = Buffer.from(typeof value === "bigint" ? value.toString() : String(value), "utf8");
	const tag = typeof value === "number" || typeof value === "bigint" ? "d" : "s";
	hash.update(`${tag}${text.length}:`).update(text);
}

function canonicalTableManifest(db: ReadOnlyActor): BackupManifest["canonical_tables"] {
	const schemaRows = db
		.prepare(
			`SELECT name, sql FROM sqlite_schema
			 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
			 ORDER BY name`,
		)
		.all() as Array<{ name: string; sql: string | null }>;
	const virtualRoots = schemaRows
		.filter((row) => /^\s*CREATE\s+VIRTUAL\s+TABLE\b/i.test(row.sql ?? ""))
		.map((row) => row.name);
	const canonical = schemaRows.filter(
		(row) => !virtualRoots.some((root) => row.name === root || row.name.startsWith(`${root}_`)),
	);

	return canonical.map((table) => {
		const quoted = quoteIdentifier(table.name);
		const columns = db.prepare(`PRAGMA table_info(${quoted})`).all() as Array<{
			cid: number;
			name: string;
			pk: number;
		}>;
		columns.sort((left, right) => left.cid - right.cid);
		const primaryKey = columns
			.filter((column) => column.pk > 0)
			.toSorted((left, right) => left.pk - right.pk)
			.map((column) => quoteIdentifier(column.name));
		const orderBy = primaryKey.length > 0 ? primaryKey.join(", ") : "rowid";
		const hash = createHash("sha256");
		let rowCount = 0;
		for (const row of db
			.prepare(`SELECT * FROM ${quoted} ORDER BY ${orderBy}`)
			.iterate() as Iterable<Record<string, unknown>>) {
			hash.update("{");
			for (const column of columns) updateRowHash(hash, column.name, row[column.name]);
			hash.update("}");
			rowCount++;
		}
		return {
			name: table.name,
			row_count: rowCount,
			checksum_sha256: hash.digest("hex"),
		};
	});
}

function ftsManifest(db: ReadOnlyActor): BackupManifest["fts_schema"] {
	const definitions = db
		.prepare(
			`SELECT type, name, sql FROM sqlite_schema
			 WHERE name = 'memory_fts'
			    OR (type = 'trigger' AND sql LIKE '%memory_fts%')
			 ORDER BY type, name`,
		)
		.all() as Array<{ type: string; name: string; sql: string }>;
	return {
		definitions,
		sha256: createHash("sha256").update(canonicalMutationJson(definitions), "utf8").digest("hex"),
		normalization_version: NORMALIZED_SCHEMA_VERSION,
	};
}

function vectorManifest(
	db: ReadOnlyActor,
): Pick<BackupManifest, "sqlite_vec" | "active_embedding_generation_id"> {
	if (!tableExists(db, "memory_vectors")) {
		return { sqlite_vec: null, active_embedding_generation_id: null };
	}
	try {
		const artifactPath = sqliteVec.getLoadablePath();
		db.loadExtension(artifactPath);
		const version = db.prepare("SELECT vec_version() AS version").get() as
			| { version: string }
			| undefined;
		const generation = db
			.prepare(
				`SELECT model FROM memory_vectors
				 GROUP BY model ORDER BY COUNT(*) DESC, model ASC LIMIT 1`,
			)
			.get() as { model: string } | undefined;
		if (!version?.version) return { sqlite_vec: null, active_embedding_generation_id: null };
		return {
			sqlite_vec: {
				version: version.version,
				artifact_sha256: sha256File(artifactPath),
				platform: `${process.platform}-${process.arch}`,
			},
			active_embedding_generation_id: generation?.model ?? null,
		};
	} catch {
		return { sqlite_vec: null, active_embedding_generation_id: null };
	}
}

type ArtifactManifestFields = Pick<
	BackupManifest,
	| "schema_version"
	| "sqlite_source_version"
	| "fts_schema"
	| "sqlite_vec"
	| "active_embedding_generation_id"
	| "canonical_tables"
	| "created_watermark"
>;

function inspectBackupArtifact(path: string): ArtifactManifestFields {
	const db = ReadOnlyActor.open(path);
	try {
		const sqlite = db.prepare("SELECT sqlite_version() AS version").get() as {
			version: string;
		};
		const schemaVersion = Number(db.pragma("user_version", { simple: true }));
		const watermark = tableExists(db, "raw_events")
			? (db
					.prepare(
						`SELECT MAX(id) AS raw_event_id,
						        MAX(created_at) AS raw_event_created_at
						 FROM raw_events`,
					)
					.get() as {
					raw_event_id: number | null;
					raw_event_created_at: string | null;
				})
			: { raw_event_id: null, raw_event_created_at: null };
		return {
			schema_version: schemaVersion,
			sqlite_source_version: sqlite.version,
			fts_schema: ftsManifest(db),
			...vectorManifest(db),
			canonical_tables: canonicalTableManifest(db),
			created_watermark: watermark,
		};
	} finally {
		db.close();
	}
}

function createManifest(input: {
	artifactPath: string;
	operationId: string;
	reason: string;
	payloadHash: string;
	artifactSha256: string;
	createdAt: string;
	retentionClass: BackupRetentionClass;
}): BackupManifest {
	return {
		manifest_version: 1,
		operation_id: input.operationId,
		reason: input.reason,
		payload_hash: input.payloadHash,
		artifact_sha256: input.artifactSha256,
		...inspectBackupArtifact(input.artifactPath),
		created_at: input.createdAt,
		retention_class: input.retentionClass,
		privacy: {
			may_contain_private_or_local_only: true,
			off_device_export: "not_available_in_phase_1",
		},
	};
}

function writeManifestSidecar(path: string, manifest: BackupManifest): BackupSidecarV2 {
	const sidecar: BackupSidecarV2 = {
		version: 2,
		manifest,
		manifest_hash: manifestHash(manifest),
		authenticity: "hash-only",
		signature: null,
	};
	durableReplaceFile(path, `${JSON.stringify(sidecar)}\n`);
	return sidecar;
}

function removeInterruptedBackupTemp(destinationDir: string, operationId: string): void {
	const path = join(destinationDir, `${operationId}.tmp`);
	let info: ReturnType<typeof lstatSync>;
	try {
		info = lstatSync(path);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
		throw error;
	}
	if (info.isSymbolicLink() || !info.isFile()) {
		throw new Error("Interrupted backup temp must be a regular file.");
	}
	unlinkSync(path);
	fsyncPath(destinationDir);
}

function manifestSnapshotDiagnostics(manifest: BackupManifest, artifactPath: string): string[] {
	let actual: ArtifactManifestFields;
	try {
		actual = inspectBackupArtifact(artifactPath);
	} catch (error) {
		return [error instanceof Error ? error.message : "backup manifest could not be verified"];
	}
	const diagnostics: string[] = [];
	if (actual.schema_version !== manifest.schema_version) {
		diagnostics.push("backup manifest schema version mismatch");
	}
	if (
		canonicalMutationJson(actual.fts_schema.definitions) !==
		canonicalMutationJson(manifest.fts_schema.definitions)
	) {
		diagnostics.push("backup manifest FTS schema mismatch");
	}
	if (actual.fts_schema.sha256 !== manifest.fts_schema.sha256) {
		diagnostics.push("backup manifest FTS hash mismatch");
	}
	if (
		canonicalMutationJson(actual.canonical_tables) !==
		canonicalMutationJson(manifest.canonical_tables)
	) {
		diagnostics.push("backup manifest canonical row mismatch");
	}
	if (
		canonicalMutationJson(actual.created_watermark) !==
		canonicalMutationJson(manifest.created_watermark)
	) {
		diagnostics.push("backup manifest watermark mismatch");
	}
	return diagnostics;
}

export function verifyOnlineBackup(input: {
	artifactPath: string;
	expectedSha256?: string;
	sourcePath?: string;
}): BackupCheck {
	if (!existsSync(input.artifactPath)) {
		return { valid: false, manifestHash: null, diagnostics: ["backup artifact is missing"] };
	}
	try {
		const info = lstatSync(input.artifactPath);
		if (info.isSymbolicLink() || !info.isFile()) {
			return {
				valid: false,
				manifestHash: null,
				diagnostics: ["backup artifact must be a regular file"],
			};
		}
		if (existsSync(`${input.artifactPath}-wal`) || existsSync(`${input.artifactPath}-shm`)) {
			return {
				valid: false,
				manifestHash: null,
				diagnostics: ["backup artifact must not have WAL sidecars"],
			};
		}
		if (input.sourcePath) {
			try {
				const source = lstatSync(input.sourcePath);
				if (source.dev === info.dev && source.ino === info.ino) {
					return {
						valid: false,
						manifestHash: null,
						diagnostics: ["backup artifact must not be the live database"],
					};
				}
			} catch {
				// source may be gone; artifact still has to stand alone
			}
		}
	} catch {
		return { valid: false, manifestHash: null, diagnostics: ["backup artifact is missing"] };
	}
	const manifestHash = sha256File(input.artifactPath);
	if (input.expectedSha256 && manifestHash !== input.expectedSha256) {
		return { valid: false, manifestHash, diagnostics: ["backup hash mismatch"] };
	}
	let db: ReadOnlyActor;
	try {
		db = ReadOnlyActor.open(input.artifactPath);
	} catch (error) {
		return {
			valid: false,
			manifestHash,
			diagnostics: [error instanceof Error ? error.message : "backup artifact could not be opened"],
		};
	}
	try {
		const rows = db.pragma("integrity_check") as Array<Record<string, unknown>>;
		if (rows.length !== 1 || Object.values(rows[0] ?? {})[0] !== "ok") {
			return { valid: false, manifestHash, diagnostics: ["sqlite integrity check failed"] };
		}
	} finally {
		db.close();
	}
	return { valid: true, manifestHash, diagnostics: [] };
}

let backupQueue: Promise<unknown> = Promise.resolve();
let pendingBackups = 0;

function enqueueBackup<T>(work: () => Promise<T>): Promise<T> {
	pendingBackups++;
	const run = backupQueue.then(work, work);
	const tracked = run.finally(() => {
		pendingBackups--;
	});
	backupQueue = tracked.then(
		() => undefined,
		() => undefined,
	);
	return tracked;
}

export async function createOnlineBackup(input: {
	db: Backupable;
	destinationDir: string;
	operationId: string;
	reason: string;
	payloadHash?: string;
	retentionClass?: BackupRetentionClass;
	now?: () => Date;
}): Promise<VerifiedBackup> {
	return enqueueBackup(() => createOnlineBackupUnlocked(input));
}

async function createOnlineBackupUnlocked(input: {
	db: Backupable;
	destinationDir: string;
	operationId: string;
	reason: string;
	payloadHash?: string;
	retentionClass?: BackupRetentionClass;
	now?: () => Date;
}): Promise<VerifiedBackup> {
	validateOperationId(input.operationId);
	if (typeof input.reason !== "string" || input.reason.length === 0) {
		throw new BackupRequestError("invalid_request", "Backup reason is required.");
	}
	const payloadHash = backupPayloadHash(input.reason);
	if (input.payloadHash !== undefined && input.payloadHash !== payloadHash) {
		throw new BackupRequestError("invalid_request", "Backup payloadHash does not match reason.");
	}

	ensurePrivateDirectory(input.destinationDir);
	removeInterruptedBackupTemp(input.destinationDir, input.operationId);
	const artifactPath = backupArtifactPath(input.destinationDir, input.operationId);
	const sidecarFile = backupSidecarPath(input.destinationDir, input.operationId);
	const sourcePath = input.db.name ?? "";
	const existing = readSidecar(sidecarFile);
	if (existing && existing.manifest.payload_hash !== payloadHash) {
		throw new BackupRequestError(
			"conflict",
			"Backup operation ID already exists with a different payload.",
		);
	}
	if (existing && existsSync(artifactPath)) {
		const check = verifyOnlineBackup({
			artifactPath,
			expectedSha256: existing.manifest.artifact_sha256,
			sourcePath: sourcePath || undefined,
		});
		if (!check.valid || !check.manifestHash) {
			throw new Error("Existing backup failed verification.");
		}
		if (
			existing.manifest_hash !== manifestHash(existing.manifest) ||
			manifestSnapshotDiagnostics(existing.manifest, artifactPath).length > 0
		) {
			throw new Error("Existing backup manifest failed verification.");
		}
		return {
			verified: true,
			evidence: `existing-online-backup:${input.operationId}:${existing.manifest_hash}`,
			backupId: input.operationId,
			artifactPath,
			artifactSha256: check.manifestHash,
			manifestHash: existing.manifest_hash,
		};
	}

	const temporaryPath = join(input.destinationDir, `${input.operationId}.tmp`);
	try {
		await input.db.backup(temporaryPath);
		finalizeStandaloneBackup(temporaryPath);
		chmodSync(temporaryPath, 0o600);
		fsyncPath(temporaryPath);
		renameSync(temporaryPath, artifactPath);
		chmodSync(artifactPath, 0o600);
		fsyncPath(artifactPath);
		fsyncPath(input.destinationDir);
	} catch (error) {
		try {
			unlinkSync(temporaryPath);
		} catch {
			// tmp may already be gone or never created
		}
		throw error;
	}

	const artifactSha256 = sha256File(artifactPath);
	const check = verifyOnlineBackup({
		artifactPath,
		expectedSha256: artifactSha256,
		sourcePath: sourcePath || undefined,
	});
	if (!check.valid || !check.manifestHash) {
		try {
			unlinkSync(artifactPath);
		} catch {
			// keep the failure; leftover must not look complete
		}
		throw new Error("Online backup failed verification.");
	}
	const manifest = createManifest({
		artifactPath,
		operationId: input.operationId,
		reason: input.reason,
		payloadHash,
		artifactSha256: check.manifestHash,
		createdAt: (input.now?.() ?? new Date()).toISOString(),
		retentionClass: input.retentionClass ?? "automatic",
	});
	const sidecar = writeManifestSidecar(sidecarFile, manifest);
	return {
		verified: true,
		evidence: `online-backup:${input.operationId}:${sidecar.manifest_hash}`,
		backupId: input.operationId,
		artifactPath,
		artifactSha256: check.manifestHash,
		manifestHash: sidecar.manifest_hash,
	};
}

function finalizeStandaloneBackup(path: string): void {
	const copy = WriterActor.open(path);
	try {
		copy.pragma("journal_mode = DELETE");
		copy.pragma("wal_checkpoint(TRUNCATE)");
	} finally {
		copy.close();
	}
	for (const suffix of ["-wal", "-shm"]) {
		try {
			unlinkSync(`${path}${suffix}`);
		} catch {
			// already absent after DELETE journal
		}
	}
}

export async function runGatedMigration(
	db: WriterActor,
	options: {
		dbPath: string;
		destinationDir: string;
		operationId: string;
		reason: string;
	},
): Promise<void> {
	const kind = peekMigrationKind(db);
	if (!kind) return;

	if (kind === "bootstrap") {
		runDatabaseMigrations(db, { dbPath: options.dbPath, backupAndVerify: verifyFreshDatabase });
		return;
	}

	const proof = await createOnlineBackup({
		db,
		destinationDir: options.destinationDir,
		operationId: options.operationId,
		reason: options.reason,
	});
	requireVerifiedBackup(proof);
	const recheck = verifyOnlineBackup({
		artifactPath: proof.artifactPath,
		expectedSha256: proof.artifactSha256,
		sourcePath: db.name,
	});
	if (!recheck.valid) {
		throw new Error("Destructive operation requires a verified backup.");
	}
	runDatabaseMigrations(db, {
		dbPath: options.dbPath,
		backupAndVerify: () => ({ verified: true, evidence: proof.evidence }),
	});
}

export async function createCanonicalBackup(input: {
	dataDir: string;
	operationId: string;
	reason: string;
	payloadHash: string;
	retentionClass?: BackupRetentionClass;
}): Promise<VerifiedBackup> {
	const layout = resolveStorageLayout(input.dataDir);
	const pointer = readCurrentDatabasePointer(layout);
	if (!pointer) {
		throw new BackupRequestError("not_found", "No canonical database to back up.");
	}
	const sourcePath = join(layout.dbDir, pointer);
	const source = ReadOnlyActor.open(sourcePath);
	try {
		return await createOnlineBackup({
			db: source,
			destinationDir: layout.backupsDir,
			operationId: input.operationId,
			reason: input.reason,
			payloadHash: input.payloadHash,
			retentionClass: input.retentionClass ?? "manual",
		});
	} finally {
		source.close();
	}
}

export function recoverCanonicalBackupResult(input: {
	dataDir: string;
	operationId: string;
	payloadHash: string;
}): VerifiedBackup | null {
	validateOperationId(input.operationId);
	const layout = resolveStorageLayout(input.dataDir);
	const sidecar = readSidecar(backupSidecarPath(layout.backupsDir, input.operationId));
	if (!sidecar) return null;
	if (sidecar.manifest.payload_hash !== input.payloadHash) {
		throw new BackupRequestError(
			"conflict",
			"Backup operation ID already exists with a different payload.",
		);
	}
	const check = verifyCanonicalBackup({ dataDir: input.dataDir, backupId: input.operationId });
	if (!check.valid || !check.manifestHash) return null;
	return {
		verified: true,
		evidence: `recovered-online-backup:${input.operationId}:${check.manifestHash}`,
		backupId: input.operationId,
		artifactPath: backupArtifactPath(layout.backupsDir, input.operationId),
		artifactSha256: sidecar.manifest.artifact_sha256,
		manifestHash: check.manifestHash,
	};
}

export function verifyCanonicalBackup(input: {
	dataDir: string;
	backupId: string;
}): BackupCheck & { backupId: string } {
	validateOperationId(input.backupId);
	const layout = resolveStorageLayout(input.dataDir);
	const artifactPath = backupArtifactPath(layout.backupsDir, input.backupId);
	const sidecarPath = backupSidecarPath(layout.backupsDir, input.backupId);
	const sidecar = readSidecar(sidecarPath);
	if (!sidecar) {
		return {
			backupId: input.backupId,
			valid: false,
			manifestHash: null,
			diagnostics: [
				existsSync(sidecarPath) ? "backup sidecar is malformed" : "backup sidecar is missing",
			],
		};
	}
	const diagnostics: string[] = [];
	if (sidecar.manifest.operation_id !== input.backupId) {
		diagnostics.push("backup manifest operation ID mismatch");
	}
	if (manifestHash(sidecar.manifest) !== sidecar.manifest_hash) {
		diagnostics.push("backup manifest hash mismatch");
	}
	for (const [path, label, expectedMode] of [
		[layout.backupsDir, "backup directory", 0o700],
		[artifactPath, "backup artifact", 0o600],
		[sidecarPath, "backup sidecar", 0o600],
	] as const) {
		try {
			if ((lstatSync(path).mode & 0o777) !== expectedMode) {
				diagnostics.push(`${label} is not owner-only`);
			}
		} catch {
			diagnostics.push(`${label} is missing`);
		}
	}
	const pointer = readCurrentDatabasePointer(layout);
	const sourcePath = pointer ? join(layout.dbDir, pointer) : undefined;
	const check = verifyOnlineBackup({
		artifactPath,
		expectedSha256: sidecar.manifest.artifact_sha256,
		sourcePath,
	});
	diagnostics.push(...check.diagnostics);
	if (check.valid) diagnostics.push(...manifestSnapshotDiagnostics(sidecar.manifest, artifactPath));
	return {
		backupId: input.backupId,
		valid: diagnostics.length === 0,
		manifestHash: sidecar.manifest_hash,
		diagnostics,
	};
}

export function listCanonicalBackups(dataDir: string): BackupListEntry[] {
	const layout = resolveStorageLayout(dataDir);
	ensurePrivateDirectory(layout.backupsDir);
	return readdirSync(layout.backupsDir, { withFileTypes: true })
		.filter((entry) => entry.name.endsWith(".json"))
		.map((entry): BackupListEntry | null => {
			const backupId = entry.name.slice(0, -5);
			if (!OPERATION_ID.test(backupId)) return null;
			const sidecar = readSidecar(join(layout.backupsDir, entry.name));
			const check = verifyCanonicalBackup({ dataDir: layout.dataDir, backupId });
			return {
				backupId,
				reason: sidecar?.manifest.reason ?? null,
				createdAt: sidecar?.manifest.created_at ?? null,
				retentionClass: sidecar?.manifest.retention_class ?? null,
				valid: check.valid,
				manifestHash: check.manifestHash,
				artifactSha256: sidecar?.manifest.artifact_sha256 ?? null,
				diagnostics: check.diagnostics,
			};
		})
		.filter((entry): entry is BackupListEntry => entry !== null)
		.sort((left, right) => {
			const created = (right.createdAt ?? "").localeCompare(left.createdAt ?? "");
			return created || left.backupId.localeCompare(right.backupId);
		});
}

function utcWeekKey(value: string): string | null {
	const date = new Date(value);
	if (!Number.isFinite(date.getTime())) return null;
	const midnight = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate()));
	const daysAfterMonday = (midnight.getUTCDay() + 6) % 7;
	midnight.setUTCDate(midnight.getUTCDate() - daysAfterMonday);
	return midnight.toISOString().slice(0, 10);
}

export function pruneBackupRetention(destinationDir: string): {
	removed: string[];
	kept: string[];
} {
	ensurePrivateDirectory(destinationDir);
	const automatic = readdirSync(destinationDir, { withFileTypes: true })
		.filter((entry) => entry.isFile() && entry.name.endsWith(".json"))
		.map((entry) => {
			const backupId = entry.name.slice(0, -5);
			const sidecar = OPERATION_ID.test(backupId)
				? readSidecar(join(destinationDir, entry.name))
				: null;
			return sidecar?.manifest.retention_class === "automatic"
				? { backupId, createdAt: sidecar.manifest.created_at }
				: null;
		})
		.filter((entry): entry is { backupId: string; createdAt: string } => entry !== null)
		.toSorted((left, right) => right.createdAt.localeCompare(left.createdAt));

	const keep = new Set<string>();
	const days = new Set<string>();
	for (const entry of automatic) {
		const day = entry.createdAt.slice(0, 10);
		if (days.has(day) || days.size >= 7) continue;
		days.add(day);
		keep.add(entry.backupId);
	}
	const weeks = new Set<string>();
	for (const entry of automatic) {
		if (keep.has(entry.backupId) || days.has(entry.createdAt.slice(0, 10)) || weeks.size >= 4) {
			continue;
		}
		const week = utcWeekKey(entry.createdAt);
		if (!week || weeks.has(week)) continue;
		weeks.add(week);
		keep.add(entry.backupId);
	}

	const removed: string[] = [];
	for (const entry of automatic) {
		if (keep.has(entry.backupId)) continue;
		durableRemoveFile(backupSidecarPath(destinationDir, entry.backupId));
		durableRemoveFile(backupArtifactPath(destinationDir, entry.backupId));
		removed.push(entry.backupId);
	}
	return { removed, kept: [...keep].toSorted() };
}

export async function createDailyBackup(input: {
	db: Backupable;
	destinationDir: string;
	now?: () => Date;
}): Promise<VerifiedBackup> {
	const now = input.now?.() ?? new Date();
	const operationId = `daily-${now.toISOString().slice(0, 10)}`;
	const proof = await createOnlineBackup({
		db: input.db,
		destinationDir: input.destinationDir,
		operationId,
		reason: "daily",
		retentionClass: "automatic",
		now: () => now,
	});
	pruneBackupRetention(input.destinationDir);
	return proof;
}

export function restorePayloadHash(backupId: string): string {
	return createHash("sha256").update(canonicalMutationJson({ backupId }), "utf8").digest("hex");
}

function restoreTarget(input: { dataDir: string; operationId: string; payloadHash: string }): {
	layout: ReturnType<typeof resolveStorageLayout>;
	operationToken: string;
	pointerName: string;
	pointer: string;
	destination: string;
	resultPath: string;
} {
	const layout = resolveStorageLayout(input.dataDir);
	const operationToken = createHash("sha256")
		.update(input.operationId, "utf8")
		.digest("hex")
		.slice(0, 32);
	const pointerName = `restore-${operationToken}-${input.payloadHash.slice(0, 16)}.sqlite`;
	const pointer = `versions/${pointerName}`;
	const destination = join(layout.versionsDir, pointerName);
	return {
		layout,
		operationToken,
		pointerName,
		pointer,
		destination,
		resultPath: `${destination}.restore.json`,
	};
}

function removeStagingArtifact(path: string): void {
	for (const candidate of [path, `${path}-wal`, `${path}-shm`, `${path}.restore.json`]) {
		try {
			durableRemoveFile(candidate);
		} catch {
			// Preserve the restore failure; an unreferenced staging file is safe to retry.
		}
	}
}

function readRestoreResult(path: string): RestoreResultRecord | null {
	if (!existsSync(path)) return null;
	const info = lstatSync(path);
	if (!info.isFile() || info.isSymbolicLink()) {
		throw new BackupRequestError("conflict", "Restore result is not a regular file.");
	}
	let value: unknown;
	try {
		value = JSON.parse(readFileSync(path, "utf8"));
	} catch {
		throw new BackupRequestError("conflict", "Restore result is unreadable.");
	}
	const record = value as Partial<RestoreResultRecord>;
	if (
		record.version !== 1 ||
		typeof record.payloadHash !== "string" ||
		!SHA256.test(record.payloadHash) ||
		typeof record.artifactSha256 !== "string" ||
		!SHA256.test(record.artifactSha256) ||
		typeof record.manifestHash !== "string" ||
		!SHA256.test(record.manifestHash)
	) {
		throw new BackupRequestError("conflict", "Restore result is malformed.");
	}
	return record as RestoreResultRecord;
}

function completedRestoreResult(
	input: { operationId: string; backupId: string; payloadHash: string },
	target: ReturnType<typeof restoreTarget>,
	result: RestoreResultRecord | null,
): RestoreBackupResult | null {
	if (readCurrentDatabasePointer(target.layout) !== target.pointer) return null;
	if (!result) throw new BackupRequestError("conflict", "Restore result is missing.");
	return {
		operationId: input.operationId,
		backupId: input.backupId,
		pointer: target.pointer,
		artifactSha256: result.artifactSha256,
		manifestHash: result.manifestHash,
		restartRequired: true,
	};
}

export function recoverCanonicalRestoreResult(input: {
	dataDir: string;
	operationId: string;
	payloadHash: string;
	backupId: string;
}): RestoreBackupResult | null {
	validateOperationId(input.operationId);
	validateOperationId(input.backupId);
	if (input.payloadHash !== restorePayloadHash(input.backupId)) {
		throw new BackupRequestError("invalid_request", "Restore payloadHash does not match backupId.");
	}
	const target = restoreTarget(input);
	const result = readRestoreResult(target.resultPath);
	if (result && result.payloadHash !== input.payloadHash) {
		throw new BackupRequestError(
			"conflict",
			"Restore operation ID already exists with a different payload.",
		);
	}
	const completed = completedRestoreResult(input, target, result);
	if (
		completed &&
		(existsSync(`${target.destination}-wal`) || existsSync(`${target.destination}-shm`))
	) {
		throw new BackupRequestError(
			"conflict",
			"Restore artifact has WAL sidecars during durable recovery.",
		);
	}
	if (completed && sha256File(target.destination) !== completed.artifactSha256) {
		throw new BackupRequestError(
			"conflict",
			"Restore artifact hash mismatch during durable recovery.",
		);
	}
	return completed;
}

function rebuildStagedDerivedIndexes(path: string, manifest: BackupManifest): void {
	const db = WriterActor.open(path);
	try {
		db.pragma("journal_mode = DELETE");
		if (tableExists(db, "memory_fts") && tableExists(db, "memory_items")) {
			db.exec("INSERT INTO memory_fts(memory_fts) VALUES('rebuild')");
		}
		if (manifest.sqlite_vec && tableExists(db, "memory_vectors")) {
			try {
				const vectorArtifact = sqliteVec.getLoadablePath();
				if (sha256File(vectorArtifact) === manifest.sqlite_vec.artifact_sha256) {
					db.loadExtension(vectorArtifact);
					db.exec("DELETE FROM memory_vectors");
				}
			} catch {
				// Vector is derived; FTS-only restore must remain independently available.
			}
		}
		const rows = db.pragma("integrity_check") as Array<Record<string, unknown>>;
		if (rows.length !== 1 || Object.values(rows[0] ?? {})[0] !== "ok") {
			throw new Error("Restored SQLite artifact failed integrity check.");
		}
	} finally {
		db.close();
	}
	finalizeStandaloneBackup(path);
	chmodSync(path, 0o600);
	fsyncPath(path);
}

export function restoreCanonicalBackup(input: {
	dataDir: string;
	operationId: string;
	payloadHash: string;
	backupId: string;
}): RestoreBackupResult {
	validateOperationId(input.operationId);
	validateOperationId(input.backupId);
	if (input.payloadHash !== restorePayloadHash(input.backupId)) {
		throw new BackupRequestError("invalid_request", "Restore payloadHash does not match backupId.");
	}
	const target = restoreTarget(input);
	const hasConflict = readdirSync(target.layout.versionsDir).some(
		(name) =>
			name.startsWith(`restore-${target.operationToken}-`) &&
			(name.endsWith(".sqlite") || name.endsWith(".sqlite.restore.json")) &&
			name !== target.pointerName &&
			name !== `${target.pointerName}.restore.json`,
	);
	if (hasConflict) {
		throw new BackupRequestError(
			"conflict",
			"Restore operation ID already exists with a different payload.",
		);
	}
	const previousResult = readRestoreResult(target.resultPath);
	if (previousResult && previousResult.payloadHash !== input.payloadHash) {
		throw new BackupRequestError(
			"conflict",
			"Restore operation ID already exists with a different payload.",
		);
	}
	if (pendingBackups > 0) {
		throw new BackupRequestError(
			"conflict",
			"A backup is active; retry restore after it completes.",
		);
	}
	const completed = completedRestoreResult(input, target, previousResult);
	if (completed) return completed;

	const check = verifyCanonicalBackup({ dataDir: input.dataDir, backupId: input.backupId });
	if (!check.valid || !check.manifestHash) {
		throw new BackupRequestError("invalid_request", "Backup failed restore verification.");
	}
	const sidecar = readSidecar(backupSidecarPath(target.layout.backupsDir, input.backupId));
	if (!sidecar) {
		throw new BackupRequestError("invalid_request", "Backup manifest is not restorable.");
	}
	if (sidecar.manifest.schema_version < MIN_WRITABLE_SCHEMA) {
		throw new BackupRequestError(
			"invalid_request",
			"Backup manifest requires the schema 20 bridge before restore.",
		);
	}
	if (
		sidecar.manifest.schema_version > SCHEMA_VERSION ||
		sidecar.manifest.fts_schema.normalization_version !== NORMALIZED_SCHEMA_VERSION
	) {
		throw new BackupRequestError("invalid_request", "Backup manifest is incompatible.");
	}

	let staged = false;
	if (existsSync(target.destination)) {
		const info = lstatSync(target.destination);
		staged = info.isFile() && !info.isSymbolicLink();
		if (!staged) removeStagingArtifact(target.destination);
	}
	if (staged && manifestSnapshotDiagnostics(sidecar.manifest, target.destination).length > 0) {
		removeStagingArtifact(target.destination);
		staged = false;
	}
	if (!staged) {
		try {
			durableCopyFile(
				backupArtifactPath(target.layout.backupsDir, input.backupId),
				target.destination,
			);
			rebuildStagedDerivedIndexes(target.destination, sidecar.manifest);
			const diagnostics = manifestSnapshotDiagnostics(sidecar.manifest, target.destination);
			if (diagnostics.length > 0) throw new Error(diagnostics.join("; "));
		} catch (error) {
			removeStagingArtifact(target.destination);
			throw error;
		}
	}

	const artifactSha256 = sha256File(target.destination);
	durableReplaceFile(
		target.resultPath,
		`${JSON.stringify({
			version: 1,
			payloadHash: input.payloadHash,
			artifactSha256,
			manifestHash: sidecar.manifest_hash,
		} satisfies RestoreResultRecord)}\n`,
	);
	activateDatabaseArtifact(target.layout, {
		operationId: input.operationId,
		pointer: target.pointer,
		artifactSha256,
	});
	return {
		operationId: input.operationId,
		backupId: input.backupId,
		pointer: target.pointer,
		artifactSha256,
		manifestHash: sidecar.manifest_hash,
		restartRequired: true,
	};
}
