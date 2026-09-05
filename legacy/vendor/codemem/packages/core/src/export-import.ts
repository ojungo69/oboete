import { readFileSync } from "node:fs";
import { drizzle } from "drizzle-orm/better-sqlite3";
import { assertSchemaReady, type Database, fromJson, toJson, toJsonNullable } from "./db.js";
import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileUntrustedDestinationBoundary,
	type DestinationBoundaryV1,
	memoryDestinationBoundarySql,
} from "./destination-boundary.js";
import { buildFilterClausesWithContext } from "./filters.js";
import { expandUserPath } from "./observer-config.js";
import { projectColumnClause, resolveProject as resolveProjectName } from "./project.js";
import * as schema from "./schema.js";
import { LOCAL_DEFAULT_SCOPE_ID } from "./scope-resolution.js";
import { resolveSessionScopeId } from "./scope-stamping.js";
import { isOneOf, trimEndWhere } from "./text-trim.js";
import type { SensitivityV1, SourceSpanV1 } from "./types.js";

const TRAILING_SLASH = isOneOf("/");

type JsonObject = Record<string, unknown>;
type MemoryInsert = typeof schema.memoryItems.$inferInsert;

interface ScopeFilter {
	clauses: string[];
	params: unknown[];
}

export interface ExportOptions {
	dbPath?: string;
	project?: string | null;
	allProjects?: boolean;
	includeInactive?: boolean;
	since?: string | null;
	cwd?: string;
}

export interface ImportOptions {
	dbPath?: string;
	remapProject?: string | null;
	dryRun?: boolean;
}

export interface ExportPayload {
	version: "1.0" | "2.0";
	exported_at: string;
	export_metadata: {
		tool_version: "codemem";
		projects: string[];
		total_memories: number;
		total_sessions: number;
		include_inactive: boolean;
		filters: JsonObject;
	};
	sessions: JsonObject[];
	memory_items: JsonObject[];
	session_summaries: JsonObject[];
	user_prompts: JsonObject[];
}

export interface ImportResult {
	sessions: number;
	user_prompts: number;
	memory_items: number;
	session_summaries: number;
	dryRun: boolean;
}

const SUMMARY_METADATA_KEYS = [
	"request",
	"investigated",
	"learned",
	"completed",
	"next_steps",
	"notes",
	"files_read",
	"files_modified",
	"prompt_number",
	"request_original",
	"discovery_tokens",
	"discovery_source",
] as const;

const REPOSITORY_IDENTITY = /^repo-v1:sha256:[a-f0-9]{64}$/;
const FINGERPRINT = /^sha256:[a-f0-9]{64}$/;
const SENSITIVITY_RANK: Record<SensitivityV1, number> = {
	eligible: 0,
	local_only: 1,
	private: 2,
	secret: 3,
};

function nowIso(): string {
	return new Date().toISOString();
}

function nowEpochMs(): number {
	return Date.now();
}

function cleanString(value: unknown): string | null {
	if (typeof value !== "string") return null;
	const trimmed = value.trim();
	return trimmed || null;
}

function boundedString(value: unknown, maxBytes = 256): string | null {
	const cleaned = cleanString(value);
	return cleaned && Buffer.byteLength(cleaned, "utf8") <= maxBytes ? cleaned : null;
}

function resolveLocalDeviceId(db: Database): string {
	const envDeviceId = cleanString(process.env.CODEMEM_DEVICE_ID);
	if (envDeviceId) return envDeviceId;
	try {
		const row = db.prepare("SELECT device_id FROM sync_device LIMIT 1").get() as
			| { device_id: string | null }
			| undefined;
		return cleanString(row?.device_id) ?? "local";
	} catch {
		return "local";
	}
}

function buildScopeFilter(db: Database, destinationBoundary: DestinationBoundaryV1): ScopeFilter {
	return buildFilterClausesWithContext(null, {
		actorId: "export",
		deviceId: resolveLocalDeviceId(db),
		enforceScopeVisibility: true,
		destinationBoundary,
	});
}

function defaultExportBoundary(): DestinationBoundaryV1 {
	return compileUntrustedDestinationBoundary({
		consumer: "export",
		configurationFingerprint: CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	});
}

function validRepositoryIdentity(value: unknown): string | null {
	return typeof value === "string" && REPOSITORY_IDENTITY.test(value) ? value : null;
}

function normalizeContentSecurity(
	row: JsonObject,
	version: ExportPayload["version"],
): { sensitivity: SensitivityV1; repositoryIdentity: string | null } {
	if (version === "1.0") return { sensitivity: "secret", repositoryIdentity: null };
	const sensitivity =
		typeof row.sensitivity === "string" && Object.hasOwn(SENSITIVITY_RANK, row.sensitivity)
			? (row.sensitivity as SensitivityV1)
			: null;
	const repositoryIdentity = validRepositoryIdentity(row.repository_identity);
	const repositoryWasValid = row.repository_identity == null || repositoryIdentity !== null;
	if (
		sensitivity === null ||
		!repositoryWasValid ||
		((sensitivity === "local_only" || sensitivity === "private") && repositoryIdentity === null)
	) {
		return { sensitivity: "secret", repositoryIdentity: null };
	}
	return { sensitivity, repositoryIdentity };
}

function strongestSensitivity(left: SensitivityV1, right: SensitivityV1): SensitivityV1 {
	return SENSITIVITY_RANK[left] >= SENSITIVITY_RANK[right] ? left : right;
}

function scopeCanBeImported(db: Database, scopeId: string, deviceId: string): boolean {
	if (scopeId === LOCAL_DEFAULT_SCOPE_ID) return true;
	const row = db
		.prepare(
			`SELECT 1 AS ok
			 FROM replication_scopes rs
			 WHERE rs.scope_id = ?
			   AND rs.status = 'active'
			   AND (
				 rs.authority_type = 'local'
				 OR EXISTS (
					SELECT 1
					FROM scope_memberships sm
					WHERE sm.scope_id = rs.scope_id
					  AND sm.device_id = ?
					  AND sm.status = 'active'
					  AND sm.membership_epoch >= rs.membership_epoch
				 )
			   )
			 LIMIT 1`,
		)
		.get(scopeId, deviceId) as { ok: number } | undefined;
	return row != null;
}

function parseDbObject(raw: unknown): unknown {
	if (typeof raw !== "string" || raw.trim().length === 0) return null;
	try {
		return JSON.parse(raw);
	} catch {
		return fromJson(raw);
	}
}

function parseRowJsonFields<T extends JsonObject>(row: T, fields: string[]): JsonObject {
	const parsed: JsonObject = { ...row };
	for (const field of fields) {
		parsed[field] = parseDbObject(row[field]);
	}
	return parsed;
}

function normalizeImportMetadata(importMetadata: unknown): JsonObject | null {
	if (importMetadata == null) return null;
	if (typeof importMetadata === "string") {
		try {
			const parsed = JSON.parse(importMetadata) as unknown;
			return parsed != null && typeof parsed === "object" && !Array.isArray(parsed)
				? (parsed as JsonObject)
				: null;
		} catch {
			return null;
		}
	}
	return typeof importMetadata === "object" && !Array.isArray(importMetadata)
		? ({ ...(importMetadata as JsonObject) } as JsonObject)
		: null;
}

export function buildImportKey(
	source: string,
	recordType: string,
	originalId: unknown,
	parts?: { project?: string | null; createdAt?: string | null; sourceDb?: string | null },
): string {
	const values = [source, recordType, String(originalId ?? "unknown")];
	if (parts?.project) values.push(parts.project);
	if (parts?.createdAt) values.push(parts.createdAt);
	if (parts?.sourceDb) values.push(parts.sourceDb);
	return values.join("|");
}

export function mergeSummaryMetadata(metadata: JsonObject, importMetadata: unknown): JsonObject {
	const parsed = normalizeImportMetadata(importMetadata);
	if (!parsed) return metadata;
	const merged: JsonObject = { ...metadata };
	for (const key of SUMMARY_METADATA_KEYS) {
		if (!(key in parsed)) continue;
		const current = merged[key];
		let shouldFill = !(key in merged);
		if (!shouldFill) {
			if (key === "discovery_tokens" || key === "prompt_number") {
				shouldFill = current == null;
			} else if (typeof current === "string") {
				shouldFill = current.trim().length === 0;
			} else if (Array.isArray(current)) {
				shouldFill = current.length === 0;
			} else {
				shouldFill = current == null;
			}
		}
		if (shouldFill) merged[key] = parsed[key];
	}
	merged.import_metadata = importMetadata;
	return merged;
}

function normalizeImportedProject(project: unknown): string | null {
	if (typeof project !== "string") return null;
	const trimmed = project.trim();
	if (!trimmed) return null;
	if (/[\\/]/.test(trimmed)) {
		const normalized = trimEndWhere(trimmed.replaceAll("\\", "/"), TRAILING_SLASH);
		const parts = normalized.split("/");
		return parts.at(-1) || null;
	}
	return trimmed;
}

function resolveExportProject(opts: ExportOptions): string | null {
	if (opts.allProjects) return null;
	return resolveProjectName(opts.cwd ?? process.cwd(), opts.project ?? null);
}

function querySessions(
	db: Database,
	project: string | null,
	since: string | null,
	scopeFilter: ScopeFilter,
	includeInactive: boolean,
	destinationBoundary: DestinationBoundaryV1,
): JsonObject[] {
	let sql = `SELECT id, started_at, ended_at, project, tool_version, import_key, repository_identity
		FROM sessions`;
	const params: unknown[] = [];
	const clauses: string[] = [];
	if (project) {
		const clause = projectColumnClause("project", project);
		if (clause.clause) {
			clauses.push(clause.clause);
			params.push(...clause.params);
		}
	}
	if (since) {
		clauses.push("started_at >= ?");
		params.push(since);
	}
	const memoryClauses = ["memory_items.session_id = sessions.id", ...scopeFilter.clauses];
	if (!includeInactive) memoryClauses.splice(1, 0, "memory_items.active = 1");
	const promptBoundary = memoryDestinationBoundarySql(destinationBoundary, "user_prompts");
	const summaryBoundary = memoryDestinationBoundarySql(destinationBoundary, "session_summaries");
	clauses.push(`(
		EXISTS (SELECT 1 FROM memory_items WHERE ${memoryClauses.join(" AND ")})
		OR EXISTS (
			SELECT 1 FROM user_prompts
			WHERE user_prompts.session_id = sessions.id AND ${promptBoundary.clause}
		)
		OR EXISTS (
			SELECT 1 FROM session_summaries
			WHERE session_summaries.session_id = sessions.id AND ${summaryBoundary.clause}
		)
	)`);
	params.push(...scopeFilter.params, ...promptBoundary.params, ...summaryBoundary.params);
	if (clauses.length > 0) sql += ` WHERE ${clauses.join(" AND ")}`;
	sql += " ORDER BY started_at ASC";
	const rows = db.prepare(sql).all(...params) as JsonObject[];
	return rows.map((row) => ({
		...row,
		project: boundedString(normalizeImportedProject(row.project)),
		tool_version: boundedString(row.tool_version, 128),
		repository_identity: validRepositoryIdentity(row.repository_identity),
	}));
}

function fetchContentBySessionIds(
	db: Database,
	table: "session_summaries" | "user_prompts",
	sessionIds: number[],
	orderBy: string,
	destinationBoundary: DestinationBoundaryV1,
): JsonObject[] {
	if (sessionIds.length === 0) return [];
	const placeholders = sessionIds.map(() => "?").join(",");
	const destination = memoryDestinationBoundarySql(destinationBoundary, table);
	const sql = `SELECT * FROM ${table}
		WHERE session_id IN (${placeholders}) AND ${destination.clause}
		ORDER BY ${orderBy}`;
	return db.prepare(sql).all(...sessionIds, ...destination.params) as JsonObject[];
}

function exportedMemoryScopeId(row: JsonObject): string {
	const existing = cleanString(row.scope_id);
	return existing ?? LOCAL_DEFAULT_SCOPE_ID;
}

function parseMemoryExportRow(row: JsonObject): JsonObject {
	return {
		...parseRowJsonFields(row, [
			"metadata_json",
			"facts",
			"concepts",
			"files_read",
			"files_modified",
			"source_event_ids_json",
			"source_spans_json",
		]),
		scope_id: exportedMemoryScopeId(row),
	};
}

function fetchMemoryRows(
	db: Database,
	sessionIds: number[],
	scopeFilter: ScopeFilter,
	includeInactive: boolean,
): JsonObject[] {
	if (sessionIds.length === 0) return [];
	const placeholders = sessionIds.map(() => "?").join(",");
	const clauses = [`session_id IN (${placeholders})`, ...scopeFilter.clauses];
	const params: unknown[] = [...sessionIds, ...scopeFilter.params];
	if (!includeInactive) clauses.push("active = 1");
	return db
		.prepare(`SELECT * FROM memory_items WHERE ${clauses.join(" AND ")} ORDER BY created_at ASC`)
		.all(...params) as JsonObject[];
}

export function exportMemoriesWithDb(
	db: Database,
	opts: ExportOptions = {},
	destinationBoundary: DestinationBoundaryV1 = defaultExportBoundary(),
): ExportPayload {
	assertSchemaReady(db);
	const resolvedProject = resolveExportProject(opts);
	const filters: JsonObject = {};
	if (resolvedProject) filters.project = resolvedProject;
	if (opts.since) filters.since = opts.since;
	const scopeFilter = buildScopeFilter(db, destinationBoundary);

	const sessions = querySessions(
		db,
		resolvedProject,
		opts.since ?? null,
		scopeFilter,
		Boolean(opts.includeInactive),
		destinationBoundary,
	);
	const sessionIds = sessions.map((row) => Number(row.id)).filter(Number.isFinite);

	const memories = fetchMemoryRows(db, sessionIds, scopeFilter, Boolean(opts.includeInactive)).map(
		(row) => parseMemoryExportRow(row),
	);

	const summaries = fetchContentBySessionIds(
		db,
		"session_summaries",
		sessionIds,
		"created_at_epoch ASC",
		destinationBoundary,
	).map((row) => parseRowJsonFields(row, ["metadata_json", "files_read", "files_edited"]));

	const prompts = fetchContentBySessionIds(
		db,
		"user_prompts",
		sessionIds,
		"created_at_epoch ASC",
		destinationBoundary,
	).map((row) => parseRowJsonFields(row, ["metadata_json"]));

	const promptImportKeys = new Map<number, string>();
	for (const prompt of prompts) {
		if (typeof prompt.id === "number" && typeof prompt.import_key === "string") {
			promptImportKeys.set(prompt.id, prompt.import_key);
		}
	}
	for (const memory of memories) {
		if (typeof memory.user_prompt_id === "number") {
			memory.user_prompt_import_key = promptImportKeys.get(memory.user_prompt_id) ?? null;
		}
	}

	return {
		version: "2.0",
		exported_at: nowIso(),
		export_metadata: {
			tool_version: "codemem",
			projects: [...new Set(sessions.map((s) => String(s.project ?? "")).filter(Boolean))],
			total_memories: memories.length,
			total_sessions: sessions.length,
			include_inactive: Boolean(opts.includeInactive),
			filters,
		},
		sessions,
		memory_items: memories,
		session_summaries: summaries,
		user_prompts: prompts,
	};
}

export function readImportPayload(inputFile: string): ExportPayload {
	const raw =
		inputFile === "-" ? readFileSync(0, "utf8") : readFileSync(expandUserPath(inputFile), "utf8");
	const parsed = JSON.parse(raw) as unknown;
	if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) {
		throw new Error("Import payload must be a JSON object");
	}
	const payload = parsed as ExportPayload;
	if (payload.version !== "1.0" && payload.version !== "2.0") {
		throw new Error(
			`Unsupported export version: ${String((parsed as JsonObject).version ?? "unknown")}`,
		);
	}
	return payload;
}

function findImportedId(
	db: Database,
	table: "sessions" | "user_prompts" | "memory_items" | "session_summaries",
	importKey: string,
	repositoryIdentity: string | null,
): number | null {
	const row = db
		.prepare(`SELECT id FROM ${table} WHERE import_key = ? AND repository_identity IS ? LIMIT 1`)
		.get(importKey, repositoryIdentity) as { id: number } | undefined;
	return row?.id ?? null;
}

function strengthenImportedContent(
	db: Database,
	table: "user_prompts" | "session_summaries",
	id: number,
	incomingSensitivity: SensitivityV1,
): void {
	const row = db.prepare(`SELECT sensitivity FROM ${table} WHERE id = ?`).get(id) as
		| { sensitivity: SensitivityV1 }
		| undefined;
	if (!row) return;
	const sensitivity = strongestSensitivity(row.sensitivity, incomingSensitivity);
	if (sensitivity !== row.sensitivity) {
		db.prepare(`UPDATE ${table} SET sensitivity = ? WHERE id = ?`).run(sensitivity, id);
	}
}

type DrizzleDb = ReturnType<typeof drizzle>;

type NormalizedMemoryProvenance = {
	lineageId: string | null;
	revisionId: string | null;
	revisionOrdinal: number | null;
	supersedesMemoryId: number | null;
	derivationKey: string | null;
	sourceEventIds: string[] | null;
	sourceSpans: SourceSpanV1[] | null;
	manifestFingerprint: string | null;
	providerFingerprint: string | null;
	attemptFingerprint: string | null;
};

type NormalizedMemorySecurity = {
	sensitivity: SensitivityV1;
	repositoryIdentity: string | null;
	provenance: NormalizedMemoryProvenance | null;
};

function parseArray(value: unknown): unknown[] | null {
	if (Array.isArray(value)) return value;
	if (typeof value !== "string") return null;
	try {
		const parsed = JSON.parse(value) as unknown;
		return Array.isArray(parsed) ? parsed : null;
	} catch {
		return null;
	}
}

function normalizeSourceSpans(value: unknown): SourceSpanV1[] | null {
	const rawSpans = parseArray(value);
	if (!rawSpans || rawSpans.length === 0) return null;
	const spans: SourceSpanV1[] = [];
	for (const value of rawSpans) {
		if (value == null || typeof value !== "object" || Array.isArray(value)) return null;
		const span = value as JsonObject;
		if (
			typeof span.eventId !== "string" ||
			span.eventId.length === 0 ||
			!Number.isSafeInteger(span.startByte) ||
			!Number.isSafeInteger(span.endByte) ||
			Number(span.startByte) < 0 ||
			Number(span.startByte) >= Number(span.endByte)
		) {
			return null;
		}
		spans.push({
			eventId: span.eventId,
			startByte: Number(span.startByte),
			endByte: Number(span.endByte),
		});
	}
	for (let index = 1; index < spans.length; index += 1) {
		const previous = spans[index - 1] as SourceSpanV1;
		const current = spans[index] as SourceSpanV1;
		// Code-unit comparison, not localeCompare: the exporter sorted these by
		// code units for the lineage hash, and a locale-aware re-validation on
		// the importing device could disagree and silently drop provenance.
		if (
			previous.eventId > current.eventId ||
			(previous.eventId === current.eventId && previous.startByte >= current.startByte)
		) {
			return null;
		}
	}
	return spans;
}

function normalizeMemoryProvenance(row: JsonObject): NormalizedMemoryProvenance | null {
	const sourceValues = parseArray(row.source_event_ids_json);
	let sourceEventIds =
		sourceValues?.filter(
			(value): value is string => typeof value === "string" && value.length > 0,
		) ?? null;
	let sourceSpans = normalizeSourceSpans(row.source_spans_json);
	const spanEventIds = sourceSpans ? new Set(sourceSpans.map((span) => span.eventId)) : null;
	const revisionOrdinal = Number(row.revision_ordinal);
	const supersedesMemoryId =
		row.supersedes_memory_id == null ? null : Number(row.supersedes_memory_id);
	if (
		!sourceValues ||
		!sourceEventIds ||
		sourceEventIds.length === 0 ||
		sourceEventIds.length !== sourceValues.length ||
		new Set(sourceEventIds).size !== sourceEventIds.length ||
		!sourceSpans ||
		sourceSpans.length !== sourceEventIds.length ||
		!spanEventIds ||
		spanEventIds.size !== sourceSpans.length ||
		sourceEventIds.some((eventId) => !spanEventIds.has(eventId))
	) {
		sourceEventIds = null;
		sourceSpans = null;
	}
	const provenance: NormalizedMemoryProvenance = {
		lineageId: boundedString(row.lineage_id),
		revisionId: boundedString(row.revision_id),
		revisionOrdinal:
			Number.isSafeInteger(revisionOrdinal) && revisionOrdinal >= 1 ? revisionOrdinal : null,
		supersedesMemoryId:
			supersedesMemoryId !== null &&
			Number.isSafeInteger(supersedesMemoryId) &&
			supersedesMemoryId >= 1
				? supersedesMemoryId
				: null,
		derivationKey: boundedString(row.derivation_key),
		sourceEventIds,
		sourceSpans,
		manifestFingerprint:
			typeof row.manifest_fingerprint === "string" && FINGERPRINT.test(row.manifest_fingerprint)
				? row.manifest_fingerprint
				: null,
		providerFingerprint:
			typeof row.provider_fingerprint === "string" && FINGERPRINT.test(row.provider_fingerprint)
				? row.provider_fingerprint
				: null,
		attemptFingerprint:
			typeof row.attempt_fingerprint === "string" && FINGERPRINT.test(row.attempt_fingerprint)
				? row.attempt_fingerprint
				: null,
	};
	return Object.values(provenance).some((value) => value !== null) ? provenance : null;
}

function normalizeMemorySecurity(
	row: JsonObject,
	version: ExportPayload["version"],
): NormalizedMemorySecurity {
	const security = normalizeContentSecurity(row, version);
	const provenance =
		version === "2.0" && security.repositoryIdentity !== null
			? normalizeMemoryProvenance(row)
			: null;
	return { ...security, provenance };
}

type ExistingMemoryImport = {
	id: number;
	sensitivity: SensitivityV1;
	repository_identity: string | null;
	active: number;
	deleted_at: string | null;
	revision_id: string | null;
	revision_ordinal: number | null;
};

function loadExistingMemory(db: Database, id: number): ExistingMemoryImport | null {
	return (
		(db
			.prepare(
				`SELECT id, sensitivity, repository_identity, active, deleted_at, revision_id, revision_ordinal
				 FROM memory_items WHERE id = ?`,
			)
			.get(id) as ExistingMemoryImport | undefined) ?? null
	);
}

function loadLineageMemories(
	db: Database,
	lineageId: string,
	repositoryIdentity: string,
): ExistingMemoryImport[] {
	return db
		.prepare(
			`SELECT id, sensitivity, repository_identity, active, deleted_at, revision_id, revision_ordinal
			 FROM memory_items
			 WHERE lineage_id = ? AND repository_identity = ?
			 ORDER BY COALESCE(revision_ordinal, 0) DESC, id DESC`,
		)
		.all(lineageId, repositoryIdentity) as ExistingMemoryImport[];
}

function mergeExistingMemory(
	db: Database,
	existing: ExistingMemoryImport,
	incomingSensitivity: SensitivityV1,
	incomingDeletedAt: string | null,
): void {
	const sensitivity = strongestSensitivity(existing.sensitivity, incomingSensitivity);
	if (incomingDeletedAt !== null) {
		db.prepare(
			`UPDATE memory_items
			 SET sensitivity = ?, active = 0, deleted_at = COALESCE(deleted_at, ?)
			 WHERE id = ?`,
		).run(sensitivity, incomingDeletedAt, existing.id);
		return;
	}
	if (sensitivity !== existing.sensitivity) {
		db.prepare("UPDATE memory_items SET sensitivity = ? WHERE id = ?").run(
			sensitivity,
			existing.id,
		);
	}
}

function normalizedMemoryFields(
	security: NormalizedMemorySecurity,
	supersedesMemoryId: number | null,
): JsonObject {
	const provenance = security.provenance;
	return {
		sensitivity: security.sensitivity,
		repository_identity: security.repositoryIdentity,
		lineage_id: provenance?.lineageId ?? null,
		revision_id: provenance?.revisionId ?? null,
		revision_ordinal: provenance?.revisionOrdinal ?? null,
		supersedes_memory_id: supersedesMemoryId,
		derivation_key: provenance?.derivationKey ?? null,
		source_event_ids_json: provenance?.sourceEventIds ?? null,
		source_spans_json: provenance?.sourceSpans ?? null,
		manifest_fingerprint: provenance?.manifestFingerprint ?? null,
		provider_fingerprint: provenance?.providerFingerprint ?? null,
		attempt_fingerprint: provenance?.attemptFingerprint ?? null,
	};
}

function insertSession(d: DrizzleDb, row: JsonObject): number {
	const rows = d
		.insert(schema.sessions)
		.values({
			started_at: typeof row.started_at === "string" ? row.started_at : nowIso(),
			ended_at: typeof row.ended_at === "string" ? row.ended_at : null,
			cwd: row.cwd == null ? null : String(row.cwd),
			project: row.project == null ? null : String(row.project),
			git_remote: row.git_remote == null ? null : String(row.git_remote),
			git_branch: row.git_branch == null ? null : String(row.git_branch),
			user: row.user == null ? null : String(row.user),
			tool_version: boundedString(row.tool_version, 128) ?? "import",
			metadata_json: toJson(row.metadata_json ?? null),
			import_key: String(row.import_key),
			repository_identity: validRepositoryIdentity(row.repository_identity),
		})
		.returning({ id: schema.sessions.id })
		.all();
	const id = rows[0]?.id;
	if (id == null) throw new Error("session insert returned no id");
	return id;
}

function insertPrompt(d: DrizzleDb, row: JsonObject): number {
	const rows = d
		.insert(schema.userPrompts)
		.values({
			session_id: Number(row.session_id),
			project: row.project == null ? null : String(row.project),
			prompt_text: String(row.prompt_text ?? ""),
			prompt_number: row.prompt_number == null ? null : Number(row.prompt_number),
			created_at: typeof row.created_at === "string" ? row.created_at : nowIso(),
			created_at_epoch:
				typeof row.created_at_epoch === "number" ? row.created_at_epoch : nowEpochMs(),
			metadata_json: toJson(row.metadata_json ?? null),
			import_key: String(row.import_key),
			sensitivity: String(row.sensitivity ?? "secret"),
			repository_identity: validRepositoryIdentity(row.repository_identity),
		})
		.returning({ id: schema.userPrompts.id })
		.all();
	const id = rows[0]?.id;
	if (id == null) throw new Error("prompt insert returned no id");
	return id;
}

function importedMemoryScopeId(db: Database, row: JsonObject, deviceId: string): string {
	const sourceScopeId = cleanString(row.scope_id);
	if (sourceScopeId) {
		if (!scopeCanBeImported(db, sourceScopeId, deviceId)) {
			throw new Error(`unauthorized_scope: ${sourceScopeId}`);
		}
		return sourceScopeId;
	}
	return resolveSessionScopeId(db, {
		sessionId: Number(row.session_id),
		workspaceId: cleanString(row.workspace_id),
	});
}

function validateImportScopes(
	db: Database,
	memories: JsonObject[],
	deviceId: string,
	remapProject: string | null,
	payloadVersion: ExportPayload["version"],
): void {
	const unauthorized = new Set<string>();
	for (const memory of memories) {
		const sourceScopeId = cleanString(memory.scope_id);
		if (!sourceScopeId) continue;
		// Skip rows that would dedupe — they don't trigger an insert, so they
		// should not block re-imports after authorization is revoked.
		const project = remapProject ?? normalizeImportedProject(memory.project);
		const memoryImportKey =
			typeof memory.import_key === "string" && memory.import_key.trim()
				? memory.import_key.trim()
				: buildImportKey("export", "memory", memory.id, {
						project,
						createdAt: typeof memory.created_at === "string" ? memory.created_at : null,
					});
		const security = normalizeMemorySecurity(memory, payloadVersion);
		if (findImportedId(db, "memory_items", memoryImportKey, security.repositoryIdentity) != null) {
			continue;
		}
		if (!scopeCanBeImported(db, sourceScopeId, deviceId)) {
			unauthorized.add(sourceScopeId);
		}
	}
	if (unauthorized.size > 0) {
		throw new Error(`unauthorized_scope: ${[...unauthorized].sort().join(", ")}`);
	}
}

function insertMemory(db: Database, d: DrizzleDb, row: JsonObject, deviceId: string): number {
	const now = nowIso();
	const parsedActive = row.active == null ? 1 : Number(row.active);
	const deletedAt =
		typeof row.deleted_at === "string" && row.deleted_at.trim().length > 0 ? row.deleted_at : null;
	const active = deletedAt === null && parsedActive === 1 ? 1 : 0;
	const workspaceId = row.workspace_id == null ? null : String(row.workspace_id);
	const scopeId = importedMemoryScopeId(db, row, deviceId);
	const values: MemoryInsert = {
		session_id: Number(row.session_id),
		kind: String(row.kind ?? "observation"),
		title: String(row.title ?? "Untitled"),
		subtitle: row.subtitle == null ? null : String(row.subtitle),
		body_text: String(row.body_text ?? row.narrative ?? ""),
		confidence: Number(row.confidence ?? 0.5),
		tags_text: String(row.tags_text ?? ""),
		active,
		created_at: typeof row.created_at === "string" ? row.created_at : now,
		updated_at: typeof row.updated_at === "string" ? row.updated_at : now,
		metadata_json: toJson(row.metadata_json ?? null),
		actor_id: row.actor_id == null ? null : String(row.actor_id),
		actor_display_name: row.actor_display_name == null ? null : String(row.actor_display_name),
		visibility: row.visibility == null ? null : String(row.visibility),
		workspace_id: workspaceId,
		workspace_kind: row.workspace_kind == null ? null : String(row.workspace_kind),
		origin_device_id: row.origin_device_id == null ? null : String(row.origin_device_id),
		origin_source: row.origin_source == null ? null : String(row.origin_source),
		trust_state: row.trust_state == null ? null : String(row.trust_state),
		facts: toJsonNullable(row.facts),
		narrative: row.narrative == null ? null : String(row.narrative),
		concepts: toJsonNullable(row.concepts),
		files_read: toJsonNullable(row.files_read),
		files_modified: toJsonNullable(row.files_modified),
		user_prompt_id: row.user_prompt_id == null ? null : Number(row.user_prompt_id),
		prompt_number: row.prompt_number == null ? null : Number(row.prompt_number),
		deleted_at: deletedAt,
		rev: Number(row.rev ?? 1),
		import_key: String(row.import_key),
		scope_id: scopeId,
		project: row.project == null ? null : String(row.project),
		sensitivity: String(row.sensitivity ?? "secret"),
		repository_identity: validRepositoryIdentity(row.repository_identity),
		lineage_id: cleanString(row.lineage_id),
		revision_id: cleanString(row.revision_id),
		revision_ordinal: row.revision_ordinal == null ? null : Number(row.revision_ordinal),
		supersedes_memory_id:
			row.supersedes_memory_id == null ? null : Number(row.supersedes_memory_id),
		derivation_key: cleanString(row.derivation_key),
		source_event_ids_json: toJsonNullable(row.source_event_ids_json),
		source_spans_json: toJsonNullable(row.source_spans_json),
		manifest_fingerprint: cleanString(row.manifest_fingerprint),
		provider_fingerprint: cleanString(row.provider_fingerprint),
		attempt_fingerprint: cleanString(row.attempt_fingerprint),
	};
	const rows = d
		.insert(schema.memoryItems)
		.values(values)
		.returning({ id: schema.memoryItems.id })
		.all();
	const id = rows[0]?.id;
	if (id == null) throw new Error("memory insert returned no id");
	return id;
}

function insertSummary(d: DrizzleDb, row: JsonObject): number {
	const rows = d
		.insert(schema.sessionSummaries)
		.values({
			session_id: Number(row.session_id),
			project: row.project == null ? null : String(row.project),
			request: String(row.request ?? ""),
			investigated: String(row.investigated ?? ""),
			learned: String(row.learned ?? ""),
			completed: String(row.completed ?? ""),
			next_steps: String(row.next_steps ?? ""),
			notes: String(row.notes ?? ""),
			files_read: toJsonNullable(row.files_read),
			files_edited: toJsonNullable(row.files_edited),
			prompt_number: row.prompt_number == null ? null : Number(row.prompt_number),
			created_at: typeof row.created_at === "string" ? row.created_at : nowIso(),
			created_at_epoch:
				typeof row.created_at_epoch === "number" ? row.created_at_epoch : nowEpochMs(),
			metadata_json: toJson(row.metadata_json ?? null),
			import_key: String(row.import_key),
			sensitivity: String(row.sensitivity ?? "secret"),
			repository_identity: validRepositoryIdentity(row.repository_identity),
		})
		.returning({ id: schema.sessionSummaries.id })
		.all();
	const id = rows[0]?.id;
	if (id == null) throw new Error("summary insert returned no id");
	return id;
}

export function importMemoriesWithDb(
	db: Database,
	payload: ExportPayload,
	opts: ImportOptions = {},
): ImportResult {
	if (payload.version !== "1.0" && payload.version !== "2.0") {
		throw new Error(`Unsupported export version: ${String(payload.version ?? "unknown")}`);
	}
	const sessionsData = Array.isArray(payload.sessions) ? payload.sessions : [];
	const memoriesData = Array.isArray(payload.memory_items) ? payload.memory_items : [];
	const summariesData = Array.isArray(payload.session_summaries) ? payload.session_summaries : [];
	const promptsData = Array.isArray(payload.user_prompts) ? payload.user_prompts : [];

	assertSchemaReady(db);
	const deviceId = resolveLocalDeviceId(db);
	validateImportScopes(db, memoriesData, deviceId, opts.remapProject ?? null, payload.version);
	if (opts.dryRun) {
		return {
			sessions: sessionsData.length,
			user_prompts: promptsData.length,
			memory_items: memoriesData.length,
			session_summaries: summariesData.length,
			dryRun: true,
		};
	}
	const d = drizzle(db, { schema });
	return db.transaction(() => {
		const sessionMapping = new Map<number, number>();
		const promptMapping = new Map<number, number>();
		const promptImportKeyMapping = new Map<string, number>();
		const memoryMapping = new Map<number, number>();
		let importedSessions = 0;
		let importedPrompts = 0;
		let importedMemories = 0;
		let importedSummaries = 0;

		for (const session of sessionsData) {
			const oldSessionId = Number(session.id);
			const project = opts.remapProject || normalizeImportedProject(session.project);
			const repositoryIdentity =
				payload.version === "2.0" ? validRepositoryIdentity(session.repository_identity) : null;
			const importKey = buildImportKey("export", "session", session.id, {
				project,
				createdAt: typeof session.started_at === "string" ? session.started_at : null,
			});
			const existingId = findImportedId(db, "sessions", importKey, repositoryIdentity);
			if (existingId != null) {
				sessionMapping.set(oldSessionId, existingId);
				continue;
			}
			const metadata: JsonObject = {
				source: "export",
				original_session_id: session.id,
				original_started_at: session.started_at ?? null,
				original_ended_at: session.ended_at ?? null,
				import_metadata: session.metadata_json ?? null,
				import_key: importKey,
			};
			// The sensitive-field scrub applies to EVERY payload version: legacy
			// v1 exports are exactly the ones that still carry real host paths
			// and usernames, so gating the scrub on v2 would skip the payload
			// class containing the PII it targets.
			const newId = insertSession(d, {
				...session,
				project,
				cwd: null,
				git_remote: null,
				git_branch: null,
				user: null,
				repository_identity: repositoryIdentity,
				metadata_json: metadata,
				import_key: importKey,
			});
			sessionMapping.set(oldSessionId, newId);
			importedSessions += 1;
		}

		for (const prompt of promptsData) {
			const oldSessionId = Number(prompt.session_id);
			const newSessionId = sessionMapping.get(oldSessionId);
			if (newSessionId == null) continue;
			const project = opts.remapProject || normalizeImportedProject(prompt.project);
			const security = normalizeContentSecurity(prompt, payload.version);
			const promptImportKey =
				typeof prompt.import_key === "string" && prompt.import_key.trim()
					? prompt.import_key.trim()
					: buildImportKey("export", "prompt", prompt.id, {
							project,
							createdAt: typeof prompt.created_at === "string" ? prompt.created_at : null,
						});
			const existingId = findImportedId(
				db,
				"user_prompts",
				promptImportKey,
				security.repositoryIdentity,
			);
			if (existingId != null) {
				strengthenImportedContent(db, "user_prompts", existingId, security.sensitivity);
				if (typeof prompt.id === "number") promptMapping.set(prompt.id, existingId);
				promptImportKeyMapping.set(promptImportKey, existingId);
				continue;
			}
			const metadata: JsonObject = {
				source: "export",
				original_prompt_id: prompt.id ?? null,
				original_created_at: prompt.created_at ?? null,
				import_metadata: prompt.metadata_json ?? null,
				import_key: promptImportKey,
			};
			const newId = insertPrompt(d, {
				...prompt,
				session_id: newSessionId,
				project,
				sensitivity: security.sensitivity,
				repository_identity: security.repositoryIdentity,
				metadata_json: metadata,
				import_key: promptImportKey,
			});
			if (typeof prompt.id === "number") promptMapping.set(prompt.id, newId);
			promptImportKeyMapping.set(promptImportKey, newId);
			importedPrompts += 1;
		}

		for (const memory of memoriesData) {
			const oldSessionId = Number(memory.session_id);
			const newSessionId = sessionMapping.get(oldSessionId);
			if (newSessionId == null) continue;
			const project = opts.remapProject || normalizeImportedProject(memory.project);
			const security = normalizeMemorySecurity(memory, payload.version);
			const memoryImportKey =
				typeof memory.import_key === "string" && memory.import_key.trim()
					? memory.import_key.trim()
					: buildImportKey("export", "memory", memory.id, {
							project,
							createdAt: typeof memory.created_at === "string" ? memory.created_at : null,
						});
			const sourceMemoryId = Number(memory.id);
			const incomingDeletedAt =
				typeof memory.deleted_at === "string" && memory.deleted_at.trim()
					? memory.deleted_at
					: null;
			const importedId = findImportedId(
				db,
				"memory_items",
				memoryImportKey,
				security.repositoryIdentity,
			);
			if (importedId !== null) {
				const existing = loadExistingMemory(db, importedId);
				if (existing) {
					mergeExistingMemory(db, existing, security.sensitivity, incomingDeletedAt);
					if (Number.isSafeInteger(sourceMemoryId)) memoryMapping.set(sourceMemoryId, existing.id);
				}
				continue;
			}

			let supersedesMemoryId: number | null = null;
			let activeToSupersede: ExistingMemoryImport | null = null;
			let insertionSensitivity = security.sensitivity;
			const provenance = security.provenance;
			if (
				provenance?.lineageId &&
				provenance.revisionId &&
				provenance.revisionOrdinal !== null &&
				security.repositoryIdentity
			) {
				const lineage = loadLineageMemories(db, provenance.lineageId, security.repositoryIdentity);
				insertionSensitivity = lineage.reduce(
					(strongest, existing) => strongestSensitivity(strongest, existing.sensitivity),
					security.sensitivity,
				);
				const exactRevision = lineage.find(
					(existing) => existing.revision_id === provenance.revisionId,
				);
				if (exactRevision) {
					mergeExistingMemory(db, exactRevision, insertionSensitivity, incomingDeletedAt);
					if (Number.isSafeInteger(sourceMemoryId)) {
						memoryMapping.set(sourceMemoryId, exactRevision.id);
					}
					continue;
				}
				const tombstone = lineage.find((existing) => existing.deleted_at !== null);
				const active = lineage.find(
					(existing) => existing.active === 1 && existing.deleted_at === null,
				);
				if (incomingDeletedAt === null && tombstone) {
					mergeExistingMemory(db, tombstone, insertionSensitivity, null);
					if (Number.isSafeInteger(sourceMemoryId)) memoryMapping.set(sourceMemoryId, tombstone.id);
					continue;
				}
				const incomingIsActive = Number(memory.active ?? 1) === 1 && incomingDeletedAt === null;
				if (incomingDeletedAt !== null && active) {
					supersedesMemoryId = active.id;
					activeToSupersede = active;
				} else if (incomingIsActive && active) {
					if ((active.revision_ordinal ?? 0) >= provenance.revisionOrdinal) {
						mergeExistingMemory(db, active, insertionSensitivity, null);
						if (Number.isSafeInteger(sourceMemoryId)) memoryMapping.set(sourceMemoryId, active.id);
						continue;
					}
					supersedesMemoryId = active.id;
					activeToSupersede = active;
				} else if (provenance.supersedesMemoryId !== null) {
					const mappedId = memoryMapping.get(provenance.supersedesMemoryId);
					const mapped = mappedId == null ? null : loadExistingMemory(db, mappedId);
					if (mapped?.repository_identity === security.repositoryIdentity) {
						supersedesMemoryId = mapped.id;
					}
				}
			}

			let linkedPromptId: number | null = null;
			if (
				typeof memory.user_prompt_import_key === "string" &&
				memory.user_prompt_import_key.trim()
			) {
				linkedPromptId = promptImportKeyMapping.get(memory.user_prompt_import_key.trim()) ?? null;
			} else if (typeof memory.user_prompt_id === "number") {
				linkedPromptId = promptMapping.get(memory.user_prompt_id) ?? null;
			}

			const baseMetadata: JsonObject = {
				source: "export",
				original_memory_id: memory.id ?? null,
				original_created_at: memory.created_at ?? null,
				import_metadata: memory.metadata_json ?? null,
				import_key: memoryImportKey,
			};
			const importedMetadata = normalizeImportMetadata(memory.metadata_json);
			if (provenance && importedMetadata) {
				for (const key of ["derived_source", "derived_stream_id"] as const) {
					const value = cleanString(importedMetadata[key]);
					if (value) baseMetadata[key] = value;
				}
			}
			if (
				typeof memory.user_prompt_import_key === "string" &&
				memory.user_prompt_import_key.trim()
			) {
				baseMetadata.user_prompt_import_key = memory.user_prompt_import_key.trim();
			}
			const metadata =
				memory.kind === "session_summary"
					? mergeSummaryMetadata(baseMetadata, memory.metadata_json ?? null)
					: baseMetadata;

			const newMemoryId = insertMemory(
				db,
				d,
				{
					...memory,
					session_id: newSessionId,
					project,
					user_prompt_id: linkedPromptId,
					metadata_json: metadata,
					import_key: memoryImportKey,
					...normalizedMemoryFields(
						{ ...security, sensitivity: insertionSensitivity },
						supersedesMemoryId,
					),
				},
				deviceId,
			);
			if (activeToSupersede) {
				db.prepare("UPDATE memory_items SET active = 0 WHERE id = ? AND active = 1").run(
					activeToSupersede.id,
				);
			}
			if (Number.isSafeInteger(sourceMemoryId)) memoryMapping.set(sourceMemoryId, newMemoryId);
			importedMemories += 1;
		}

		for (const summary of summariesData) {
			const oldSessionId = Number(summary.session_id);
			const newSessionId = sessionMapping.get(oldSessionId);
			if (newSessionId == null) continue;
			const project = opts.remapProject || normalizeImportedProject(summary.project);
			const security = normalizeContentSecurity(summary, payload.version);
			const summaryImportKey =
				typeof summary.import_key === "string" && summary.import_key.trim()
					? summary.import_key.trim()
					: buildImportKey("export", "summary", summary.id, {
							project,
							createdAt: typeof summary.created_at === "string" ? summary.created_at : null,
						});
			const existingSummaryId = findImportedId(
				db,
				"session_summaries",
				summaryImportKey,
				security.repositoryIdentity,
			);
			if (existingSummaryId !== null) {
				strengthenImportedContent(db, "session_summaries", existingSummaryId, security.sensitivity);
				continue;
			}
			const metadata: JsonObject = {
				source: "export",
				original_summary_id: summary.id ?? null,
				original_created_at: summary.created_at ?? null,
				import_metadata: summary.metadata_json ?? null,
				import_key: summaryImportKey,
			};
			insertSummary(d, {
				...summary,
				session_id: newSessionId,
				project,
				sensitivity: security.sensitivity,
				repository_identity: security.repositoryIdentity,
				metadata_json: metadata,
				import_key: summaryImportKey,
			});
			importedSummaries += 1;
		}

		return {
			sessions: importedSessions,
			user_prompts: importedPrompts,
			memory_items: importedMemories,
			session_summaries: importedSummaries,
			dryRun: false,
		};
	})();
}
