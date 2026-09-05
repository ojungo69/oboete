/**
 * MemoryStore — the main read/write surface for the codemem memory store.
 *
 * Uses the writer actor that owns the audited SQLite connection and exposes
 * CRUD, search, pack, and maintenance helpers. Schema setup is delegated to
 * the explicit migration runner before the store begins normal work.
 */

import { createHash, randomUUID } from "node:crypto";
import { statSync } from "node:fs";
import { and, desc, eq, gt, inArray, isNotNull, lte, or, type SQL, sql } from "drizzle-orm";
import { drizzle } from "drizzle-orm/better-sqlite3";
import {
	type EffectiveCapabilityManifestV1,
	PROCESSING_JOB_CAPACITY,
	PROCESSING_JOB_CONCURRENCY,
	validateCapabilityManifest,
} from "./capability-manifest.js";
import {
	assertSchemaReady,
	ensurePlannerStats,
	fromJson,
	isEmbeddingDisabled,
	loadSqliteVec,
	rawEventPayloadDigest,
	resolveDbPath,
	tableExists,
	toJson,
	toJsonNullable,
} from "./db.js";
import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileUntrustedDestinationBoundary,
	type DestinationBoundaryV1,
	type DestinationConsumerV1,
	destinationBoundaryFingerprint,
	destinationBoundarySql,
	isDestinationEligible,
	memoryDestinationBoundarySql,
} from "./destination-boundary.js";
import { buildFilterClausesWithContext, type OwnershipFilterContext } from "./filters.js";
import { buildMemoryDedupKey, normalizeMemoryDedupTitle } from "./memory-dedup.js";
import { validateMemoryKind } from "./memory-kinds.js";
import { canonicalMutationJson } from "./mutation-dispatcher.js";
import { readCodememConfigFile, readCodememConfigFileWithStatus } from "./observer-config.js";
import type { PackArtifacts } from "./pack.js";
import {
	buildMemoryPack,
	buildMemoryPackAsync,
	buildMemoryPackTrace,
	buildMemoryPackTraceAsync,
	buildMemoryPackWithTrace,
	buildMemoryPackWithTraceAsync,
} from "./pack.js";
import {
	hasExactRawEventSourceRange,
	inspectRawEventSourceRange,
} from "./raw-event-source-range.js";
import {
	prepareRedactionWorkerForScan,
	redactValueInWorker,
	warmRedactionWorker,
} from "./redaction-worker.js";
import { populateMemoryRefs } from "./ref-populate.js";
import type { RefQueryOptions, RefQueryResult } from "./ref-queries.js";
import { findByConcept as findByConceptFn, findByFile as findByFileFn } from "./ref-queries.js";
import * as schema from "./schema.js";
import { resolveVisibleScopeIds } from "./scope-resolution.js";
import { resolveSessionScopeId } from "./scope-stamping.js";
import {
	type ExplainOptions,
	explain as explainFn,
	type OwnershipCandidate,
	search as searchFn,
	timeline as timelineFn,
} from "./search.js";
import {
	loadScannerOptionsFromConfig,
	type ScanDetection,
	SecretScanner,
} from "./secret-scanner.js";
import type {
	DoctorProcessingJobProjection,
	ExplainResponse,
	MemoryFilters,
	MemoryItem,
	MemoryItemResponse,
	MemoryResult,
	PackRenderOptions,
	PackResponse,
	PackTrace,
	ProjectedSourceSetV1,
	RawEventCaptureOutcome,
	RawEventCaptureState,
	RawEventFlushBatch,
	RawEventJobAdmission,
	RawEventJobClaim,
	RawEventJobStatus,
	RawEventMemoryCompletion,
	RawEventPrivacyProjection,
	ResumeFanoutResult,
	ResumeSignalDisposition,
	ResumeSignalKind,
	ResumeSignalResult,
	ResumeSignalV1,
	SensitivityV1,
	SourceCitationV1,
	SourceSpanV1,
	StoreStats,
	TimelineItemResponse,
} from "./types.js";
import { storeVectors } from "./vectors.js";
import type { WriterActor } from "./writer-actor.js";

// Helpers

/** ISO 8601 timestamp in UTC. */
function nowIso(): string {
	return new Date().toISOString();
}

const EVENT_PAYLOAD_DIGEST_VERSION = "event-payload-digest-v1" as const;
const UNKNOWN_REPOSITORY_INDEX_SENTINEL = "repo-v1:unknown";
const REPOSITORY_IDENTITY = /^repo-v1:sha256:[a-f0-9]{64}$/;
const SAFE_FINGERPRINT = /^sha256:[a-f0-9]{64}$/;
const SENSITIVITY_RANK: Record<SensitivityV1, number> = {
	eligible: 0,
	local_only: 1,
	private: 2,
	secret: 3,
};

function sha256(value: string): string {
	return createHash("sha256").update(value, "utf8").digest("hex");
}

function stableReceiptId(prefix: string, domain: string, value: unknown): string {
	const preimage = `${domain}\0${canonicalMutationJson(value)}`;
	return `${prefix}:sha256:${sha256(preimage)}`;
}

function strongestSensitivity(left: SensitivityV1, right: SensitivityV1): SensitivityV1 {
	return SENSITIVITY_RANK[left] >= SENSITIVITY_RANK[right] ? left : right;
}

function validatedRepositoryIdentity(value: string | null | undefined): string | null {
	if (value == null) return null;
	if (!REPOSITORY_IDENTITY.test(value)) throw new Error("repository_identity is invalid");
	return value;
}

function validatedFingerprint(value: string | null | undefined, field: string): string | null {
	if (value == null) return null;
	if (!SAFE_FINGERPRINT.test(value)) throw new Error(`${field} is invalid`);
	return value;
}

function requiredFingerprint(value: string | null | undefined, field: string): string {
	const fingerprint = validatedFingerprint(value, field);
	if (fingerprint === null) throw new Error(`${field} is required`);
	return fingerprint;
}

function validatedSensitivity(value: unknown): SensitivityV1 {
	return typeof value === "string" && Object.hasOwn(SENSITIVITY_RANK, value)
		? (value as SensitivityV1)
		: "secret";
}

type RawEventCaptureInput = {
	opencodeSessionId: string;
	source?: string;
	eventId: string;
	eventType: string;
	payload: Record<string, unknown>;
	tsWallMs?: number | null;
	tsMonoMs?: number | null;
	repositoryIdentity?: string | null;
	captureManifestFingerprint?: string | null;
	sensitivity?: SensitivityV1;
	captureState?: RawEventCaptureState;
	safeErrorCode?: string | null;
	redactionDegraded?: boolean;
};

type BoundRawEventSource = {
	eventId: string;
	eventSeq: number;
	eventType: string;
	tsWallMs: number | null;
	tsMonoMs: number | null;
	payloadJson: string;
	sensitivity: SensitivityV1;
	repositoryIdentity: string | null;
	captureState: RawEventCaptureState;
	safeErrorCode: string | null;
	payloadDigestVersion: "event-payload-digest-v1";
	payloadDigest: string;
};

type BoundRawEventProjection = {
	boundary: DestinationBoundaryV1;
	set: ProjectedSourceSetV1;
	allSources: readonly BoundRawEventSource[];
	projectedSources: readonly BoundRawEventSource[];
};

const PROVIDER_AUTHORITY_FIELDS = new Set([
	"_normalized",
	"eventId",
	"event_id",
	"idempotencyKey",
	"payloadDigest",
	"payload_digest",
	"repositoryIdentity",
	"repository_identity",
	"sourceHash",
]);

function providerRedactedPayload(value: unknown): unknown {
	if (Array.isArray(value)) return value.map(providerRedactedPayload);
	if (!value || typeof value !== "object") return value;
	return Object.fromEntries(
		Object.entries(value as Record<string, unknown>)
			.filter(([key]) => !PROVIDER_AUTHORITY_FIELDS.has(key))
			.map(([key, child]) => [key, providerRedactedPayload(child)]),
	);
}

type DerivedMemoryProvenance = {
	source: string;
	streamId: string;
	repositoryIdentity: string;
	sensitivity: SensitivityV1;
	dedupSourceIdentity: string;
	sourceEventIds: string[];
	sourceSpans: SourceSpanV1[];
	lineageId: string;
	derivationKey: string;
};

type RawEventDerivedMemoryInput = {
	sourceCitations: SourceCitationV1[];
	sessionId: number;
	kind: string;
	title: string;
	bodyText: string;
	confidence?: number;
	tags?: string[];
	metadata?: Record<string, unknown>;
};

type TrustedMemoryDisposition = {
	sensitivity: SensitivityV1;
	repositoryIdentity: string | null;
};

export type RawEventDerivationContext = {
	remember(input: RawEventDerivedMemoryInput): RawEventMemoryCompletion;
};

type ResumeSignalJobRow = {
	source: string;
	stream_id: string;
	start_event_seq: number;
	end_event_seq: number;
	status: string;
	admission_manifest_fingerprint: string | null;
	admission_provider_fingerprint: string | null;
	attempt_manifest_fingerprint: string | null;
	attempt_provider_fingerprint: string | null;
	legacy_recovery_state: RawEventFlushBatch["legacy_recovery_state"];
	resume_grant_state: RawEventFlushBatch["resume_grant_state"];
	last_resume_sequence: number;
};

type ResumeProducerReceiptIdentity = {
	receiptId: string;
	producerKind: ResumeSignalKind;
	producerSequence: number;
	manifestFingerprint: string;
	providerFingerprint: string;
	targetJobId?: number;
};

type ResumeProducerReceiptRow = {
	producer_kind: ResumeSignalKind;
	configuration_fingerprint: string;
	provider_fingerprint: string;
	producer_sequence: number;
	fanout_count: number;
	target_job_ids_json: string;
};

function parseResumeProducerTargetJobIds(value: string): number[] {
	let parsed: unknown;
	try {
		parsed = JSON.parse(value);
	} catch {
		throw new ProcessingResumeError("invalid_signal", "Resume producer target set is invalid.");
	}
	if (
		!Array.isArray(parsed) ||
		parsed.length > PROCESSING_JOB_CAPACITY ||
		parsed.some((jobId) => !Number.isSafeInteger(jobId) || Number(jobId) <= 0) ||
		new Set(parsed).size !== parsed.length ||
		parsed.some((jobId, index) => index > 0 && Number(parsed[index - 1]) >= Number(jobId))
	) {
		throw new ProcessingResumeError("invalid_signal", "Resume producer target set is invalid.");
	}
	return parsed as number[];
}

function isLegacyUnknownAttempt(job: ResumeSignalJobRow | undefined): boolean {
	return (
		job?.legacy_recovery_state === "complete_range" &&
		job.admission_manifest_fingerprint === null &&
		job.admission_provider_fingerprint === null &&
		job.attempt_manifest_fingerprint === null &&
		job.attempt_provider_fingerprint === null
	);
}

type ExistingRawEventRow = {
	id: number;
	event_seq: number;
	payload_digest: string;
	sensitivity: SensitivityV1;
	capture_state: RawEventCaptureState;
	safe_error_code: string | null;
};

function resolveResumeSignalDisposition(
	input: ResumeSignalV1,
	job: ResumeSignalJobRow | undefined,
	manifestFingerprint: string,
	providerFingerprint: string,
): ResumeSignalDisposition {
	if (!job) return "wrong_job";
	if (input.targetRole !== "summary") return "wrong_role";
	if (job.status !== "retry_exhausted") return "unrelated_component";
	const legacyDoctorTarget =
		input.kind === "user_confirmed_doctor_retry" && isLegacyUnknownAttempt(job);
	if (
		input.kind !== "validated_configuration_activation" &&
		!legacyDoctorTarget &&
		job.attempt_provider_fingerprint !== providerFingerprint
	) {
		return "wrong_provider";
	}
	if (
		input.kind !== "validated_configuration_activation" &&
		!legacyDoctorTarget &&
		job.attempt_manifest_fingerprint !== manifestFingerprint
	) {
		return "unrelated_component";
	}
	if (
		input.kind === "validated_configuration_activation" &&
		job.attempt_provider_fingerprint === providerFingerprint &&
		job.attempt_manifest_fingerprint === manifestFingerprint
	) {
		return "unchanged_configuration";
	}
	return input.sequence <= Number(job.last_resume_sequence) ? "stale" : "accepted";
}

function rawEventCaptureInputFromBatch(
	opencodeSessionId: string,
	sourceName: string,
	event: Record<string, unknown>,
): RawEventCaptureInput | null {
	const eventId = String(event.event_id ?? "");
	const eventType = String(event.event_type ?? "");
	if (!eventId || !eventType) return null;
	const payload =
		event.payload && typeof event.payload === "object" && !Array.isArray(event.payload)
			? (event.payload as Record<string, unknown>)
			: {};
	return {
		opencodeSessionId,
		source: sourceName,
		eventId,
		eventType,
		payload,
		tsWallMs: typeof event.ts_wall_ms === "number" ? event.ts_wall_ms : null,
		tsMonoMs: typeof event.ts_mono_ms === "number" ? event.ts_mono_ms : null,
		repositoryIdentity:
			typeof event.repository_identity === "string" ? event.repository_identity : null,
		captureManifestFingerprint:
			typeof event.capture_manifest_fingerprint === "string"
				? event.capture_manifest_fingerprint
				: null,
		sensitivity: event.sensitivity as SensitivityV1 | undefined,
		captureState: event.capture_state as RawEventCaptureState | undefined,
		safeErrorCode: typeof event.safe_error_code === "string" ? event.safe_error_code : undefined,
		redactionDegraded: event.redaction_degraded === true,
	};
}

export class ProcessingResumeError extends Error {
	constructor(
		readonly code: "not_found" | "stale_snapshot" | "grant_pending" | "invalid_signal",
		message: string,
		readonly retryable = false,
	) {
		super(message);
		this.name = "ProcessingResumeError";
	}
}

export class ProcessingOutputLimitError extends Error {
	constructor() {
		super("output limit exceeded");
		this.name = "ProcessingOutputLimitError";
	}
}

export class StaleRawEventClaimError extends Error {
	constructor() {
		super("stale claim");
		this.name = "StaleRawEventClaimError";
	}
}

function countQuestionPlaceholders(clause: string): number {
	return (clause.match(/\?/g) ?? []).length;
}

function sqlFromParameterizedClause(clause: string, params: unknown[]): SQL {
	const parts = clause.split("?");
	let acc: SQL = sql.raw(parts[0] ?? "");
	for (let i = 1; i < parts.length; i++) {
		acc = sql`${acc}${params[i - 1]}${sql.raw(parts[i] ?? "")}`;
	}
	return acc;
}

function buildWhereSql(clauses: string[], params: unknown[]): SQL {
	const sqlClauses: SQL[] = [];
	let cursor = 0;
	for (const clause of clauses) {
		const count = countQuestionPlaceholders(clause);
		const clauseParams = params.slice(cursor, cursor + count);
		sqlClauses.push(sqlFromParameterizedClause(clause, clauseParams));
		cursor += count;
	}
	if (cursor !== params.length) {
		throw new Error("filter parameter mismatch while building SQL clauses");
	}
	if (sqlClauses.length === 1) return sqlClauses[0] ?? sql`1=1`;
	const combined = and(...sqlClauses);
	if (!combined) throw new Error("failed to combine filter SQL clauses");
	return combined;
}

/** Trim a string value, returning null for empty/non-string. Matches Python's _clean_optional_str. */
function cleanStr(value: unknown): string | null {
	if (value == null || typeof value !== "string") return null;
	const trimmed = value.trim();
	return trimmed.length > 0 ? trimmed : null;
}

const DEFAULT_CROSS_SESSION_DEDUP_WINDOW_MS = 3_600_000;
const CROSS_SESSION_DEDUP_WINDOW_ENV = "CODEMEM_MEMORY_CROSS_SESSION_DEDUP_WINDOW_MS";
const MAX_CROSS_SESSION_DEDUP_WINDOW_MS = 8_640_000_000_000_000;
const CODEMEM_DEBUG_ENV = "CODEMEM_DEBUG";
const ADVANCED_LEGACY_RECOVERY_PREDICATE = `status = 'queued'
	AND legacy_recovery_state = 'complete_range'
	AND frontier_already_advanced = 1
	AND resume_grant_state = 'pending'`;
const RAW_EVENT_PURGE_PREDICATE = `ts_wall_ms IS NOT NULL AND ts_wall_ms < ?
	AND EXISTS (
		SELECT 1 FROM raw_event_sessions s
		WHERE s.source = raw_events.source AND s.stream_id = raw_events.stream_id
		  AND raw_events.event_seq <= s.last_flushed_event_seq
	)
	AND NOT EXISTS (
		SELECT 1 FROM raw_event_flush_batches b
		WHERE b.source = raw_events.source AND b.stream_id = raw_events.stream_id
		  AND b.status IN ('queued','processing','failed','retry_exhausted')
		  AND raw_events.event_seq BETWEEN b.start_event_seq AND b.end_event_seq
	)`;

type DedupHit = {
	id: number;
	scope: "same_session" | "cross_session";
};

function getMemoryDedupMatchText(title: string): string | null {
	const normalized = normalizeMemoryDedupTitle(title);
	const fallback = title.toLowerCase().replace(/\s+/g, " ").trim();
	return normalized || fallback || null;
}

function buildDerivedMemoryDedupKey(
	title: string,
	repositoryIdentity: string,
	dedupSourceIdentity: string,
): string | null {
	const titleKey = buildMemoryDedupKey(title);
	if (titleKey === null) return null;
	return sha256(
		`free-mem:derived-memory-dedup:v1\0${repositoryIdentity}\0${dedupSourceIdentity}\0${titleKey}`,
	);
}

function isUtf8Boundary(value: string, offset: number): boolean {
	const bytes = Buffer.from(value, "utf8");
	return (
		offset === 0 ||
		offset === bytes.length ||
		(offset > 0 && offset < bytes.length && ((bytes[offset] as number) & 0xc0) !== 0x80)
	);
}

function sourceSpansEqual(left: readonly SourceSpanV1[], right: readonly SourceSpanV1[]): boolean {
	return (
		left.length === right.length &&
		left.every(
			(span, index) =>
				span.eventId === right[index]?.eventId &&
				span.startByte === right[index]?.startByte &&
				span.endByte === right[index]?.endByte,
		)
	);
}

// Overlap is judged per shared event with byte-range intersection. Requiring
// the full event-ID sets to match would let a provider bypass both gates by
// adding one unrelated event to its citation ({A} vs {A,B}) — resurrecting
// deleted content past the tombstone gate, or landing a duplicate active
// anchor whose twin survives a later forget. Per the data-model contract,
// ambiguous overlap with an active anchor rejects the response and only
// DISJOINT spans may become sibling facts; a provider citing overlapping
// spans across outputs in one batch is rejected by design.
function sourceSpansOverlap(
	left: readonly SourceSpanV1[],
	right: readonly SourceSpanV1[],
): boolean {
	return left.some((a) =>
		right.some(
			(b) => a.eventId === b.eventId && a.startByte < b.endByte && b.startByte < a.endByte,
		),
	);
}

function resolveCrossSessionDedupWindowMs(): number {
	const raw = process.env[CROSS_SESSION_DEDUP_WINDOW_ENV]?.trim();
	if (!raw) return DEFAULT_CROSS_SESSION_DEDUP_WINDOW_MS;
	const parsed = Number(raw);
	if (!Number.isFinite(parsed) || parsed < 0) return DEFAULT_CROSS_SESSION_DEDUP_WINDOW_MS;
	return Math.min(Math.floor(parsed), MAX_CROSS_SESSION_DEDUP_WINDOW_MS);
}

function isSameSessionDedupConstraintError(error: unknown): boolean {
	const message = error instanceof Error ? error.message.toLowerCase() : "";
	return (
		message.includes("idx_memory_items_same_session_dedup_unique") ||
		(message.includes("unique constraint failed") && message.includes("memory_items.session_id"))
	);
}

/**
 * Parse a row's metadata_json string into a plain object.
 * Returns a new MemoryItemResponse with metadata_json as a parsed object.
 */
function parseMetadata(row: MemoryItem): MemoryItemResponse {
	const { metadata_json, ...rest } = row;
	return { ...rest, metadata_json: fromJson(metadata_json) };
}

// MemoryStore

export class MemoryStore {
	readonly db: WriterActor;
	readonly dbPath: string;
	deviceId: string;
	actorId: string;
	actorDisplayName: string;
	readonly crossSessionDedupWindowMs: number;
	private actorIdUsesDeviceFallback: boolean;
	/**
	 * Per-store secret scanner. Lives on the instance (not as a module global)
	 * so workspace-level rule overrides and allowlists can be wired without
	 * coupling between stores in the same process. Mutable so tests and
	 * follow-on issues (codemem-ben8, codemem-jasn) can swap it.
	 */
	scanner: SecretScanner;
	private readonly pendingVectorWrites = new Set<Promise<void>>();
	private readonly rawEventClaimProjections = new WeakMap<
		RawEventJobClaim,
		BoundRawEventProjection
	>();

	/** Lazy Drizzle ORM wrapper — shares the same better-sqlite3 connection. */
	private _drizzle: ReturnType<typeof drizzle> | null = null;
	private get d() {
		this._drizzle ??= drizzle(this.db, { schema });
		return this._drizzle;
	}

	private readonly ownsConnection: boolean;

	constructor(connection: WriterActor, options: { closeConnection?: boolean } = {}) {
		warmRedactionWorker();
		this.dbPath = resolveDbPath(connection.name);
		this.ownsConnection = options.closeConnection === true;
		this.db = connection;
		try {
			loadSqliteVec(this.db);
			assertSchemaReady(this.db);
			ensurePlannerStats(this.db);
			// Read config once and use it for both actor identity and scanner
			// rule overrides. Workspace-scoped rules and allowlist live under the
			// `secret_scanner` block (see loadScannerOptionsFromConfig).
			const configRead = readCodememConfigFileWithStatus();
			const config = configRead.config;
			const scannerOptions = loadScannerOptionsFromConfig(config);
			scannerOptions.degraded ||= configRead.degraded;
			this.scanner = new SecretScanner(scannerOptions);

			// Resolve device ID: env var → sync_device table → stable "local" fallback.
			// Python uses exactly this order and fallback.
			const envDeviceId = process.env.CODEMEM_DEVICE_ID?.trim();
			if (envDeviceId) {
				this.deviceId = envDeviceId;
			} else {
				// Guard: sync_device may not exist in older/minimal schemas
				let dbDeviceId: string | undefined;
				try {
					const row = this.d
						.select({ device_id: schema.syncDevice.device_id })
						.from(schema.syncDevice)
						.limit(1)
						.get();
					dbDeviceId = row?.device_id;
				} catch {
					// Table doesn't exist — fall through to stable default
				}
				this.deviceId = dbDeviceId ?? "local";
			}

			// Resolve actor identity — matches Python load_config() precedence:
			// config file, then env override, then local fallbacks.
			const configActorId = Object.hasOwn(process.env, "CODEMEM_ACTOR_ID")
				? cleanStr(process.env.CODEMEM_ACTOR_ID)
				: (cleanStr(config.actor_id) ?? null);
			this.actorIdUsesDeviceFallback = !configActorId;
			this.actorId = configActorId || `local:${this.deviceId}`;

			const configDisplayName = Object.hasOwn(process.env, "CODEMEM_ACTOR_DISPLAY_NAME")
				? cleanStr(process.env.CODEMEM_ACTOR_DISPLAY_NAME)
				: (cleanStr(config.actor_display_name) ?? null);
			this.actorDisplayName =
				configDisplayName ||
				process.env.USER?.trim() ||
				process.env.USERNAME?.trim() ||
				this.actorId;
			this.crossSessionDedupWindowMs = resolveCrossSessionDedupWindowMs();
		} catch (err) {
			if (this.ownsConnection) this.db.close();
			throw err;
		}
	}

	hasCurrentIdentity(): boolean {
		const envDeviceId = process.env.CODEMEM_DEVICE_ID?.trim();
		let dbDeviceId: string | undefined;
		if (!envDeviceId) {
			try {
				const row = this.d
					.select({ device_id: schema.syncDevice.device_id })
					.from(schema.syncDevice)
					.limit(1)
					.get();
				dbDeviceId = row?.device_id;
			} catch {
				// Older/minimal schemas use the stable local fallback.
			}
		}
		const deviceId = envDeviceId || dbDeviceId || "local";
		const config = readCodememConfigFile();
		const configActorId = Object.hasOwn(process.env, "CODEMEM_ACTOR_ID")
			? cleanStr(process.env.CODEMEM_ACTOR_ID)
			: (cleanStr(config.actor_id) ?? null);
		const actorId = configActorId || `local:${deviceId}`;
		return deviceId === this.deviceId && actorId === this.actorId;
	}

	refreshPersistedLocalIdentity(expectedActorId: string): boolean {
		const actorId = cleanStr(expectedActorId);
		if (!actorId) return false;
		try {
			if (!tableExists(this.db, "actors")) return false;
			const actor = this.db
				.prepare(
					`SELECT display_name FROM actors
					 WHERE actor_id = ? AND is_local = 1 AND status = 'active'
					   AND merged_into_actor_id IS NULL`,
				)
				.get(actorId) as { display_name: string } | undefined;
			const displayName = cleanStr(actor?.display_name);
			if (!actor || !displayName) return false;
			this.actorId = actorId;
			this.actorDisplayName = displayName;
			this.actorIdUsesDeviceFallback = false;
			return true;
		} catch {
			return false;
		}
	}

	adoptEnsuredDeviceIdentity(deviceId: string): void {
		const normalizedDeviceId = deviceId.trim();
		if (!normalizedDeviceId || normalizedDeviceId === "local" || this.deviceId !== "local") return;
		const previousDeviceId = this.deviceId;
		const fallbackActorId = `local:${previousDeviceId}`;
		const hasActorsTable = tableExists(this.db, "actors");
		const includeInactiveParam = this.actorIdUsesDeviceFallback ? 1 : 0;
		const fallbackActor = hasActorsTable
			? (this.db
					.prepare(
						`SELECT display_name FROM actors WHERE actor_id = ?
						 AND (? = 1 OR (is_local = 1 AND status = 'active'))`,
					)
					.get(fallbackActorId, includeInactiveParam) as { display_name: string } | undefined)
			: undefined;
		if (!this.actorIdUsesDeviceFallback && !fallbackActor) {
			this.deviceId = normalizedDeviceId;
			return;
		}

		const nextActorId = this.actorIdUsesDeviceFallback
			? `local:${normalizedDeviceId}`
			: this.actorId;
		const fallbackDisplayName = cleanStr(fallbackActor?.display_name);
		const nextActorDisplayName =
			this.actorIdUsesDeviceFallback &&
			fallbackDisplayName &&
			fallbackDisplayName !== fallbackActorId
				? fallbackDisplayName
				: this.actorDisplayName === fallbackActorId
					? nextActorId
					: this.actorDisplayName;
		const now = nowIso();
		this.db.transaction(() => {
			if (fallbackActor) {
				this.db
					.prepare(
						`INSERT INTO actors(
						 actor_id, display_name, is_local, status, merged_into_actor_id, created_at, updated_at
						 ) VALUES (?, ?, 1, 'active', NULL, ?, ?)
						 ON CONFLICT(actor_id) DO UPDATE SET
						 display_name = excluded.display_name, is_local = 1, status = 'active',
						 merged_into_actor_id = NULL, updated_at = excluded.updated_at`,
					)
					.run(nextActorId, nextActorDisplayName, now, now);
			}
			this.db
				.prepare(
					// Canonicalize only rows that still carry both fallback ownership signals.
					// origin_device_id and metadata clock_device_id remain historical provenance;
					// personal workspace identity follows the canonical actor when applicable.
					`UPDATE memory_items
					 SET actor_id = ?,
					     workspace_id = CASE WHEN workspace_id = ? THEN ? ELSE workspace_id END
					 WHERE actor_id = ?
					   AND (origin_device_id IS NULL OR TRIM(origin_device_id) = '' OR origin_device_id = ?)`,
				)
				.run(
					nextActorId,
					`personal:${fallbackActorId}`,
					`personal:${nextActorId}`,
					fallbackActorId,
					previousDeviceId,
				);
			if (!fallbackActor) return;
			this.db
				.prepare(
					`UPDATE actors SET is_local = 0, status = 'merged', merged_into_actor_id = ?, updated_at = ?
					 WHERE actor_id = ?`,
				)
				.run(nextActorId, now, fallbackActorId);
		})();
		// Publish the new in-memory identity only after every persistence mutation commits.
		this.deviceId = normalizedDeviceId;
		if (this.actorIdUsesDeviceFallback) {
			this.actorId = nextActorId;
			this.actorDisplayName = nextActorDisplayName;
		}
	}

	private findExistingDuplicateMemory(
		sessionId: number,
		kind: string,
		title: string,
		dedupKey: string | null,
		scopeId: string,
		provenance: {
			visibility: string;
			workspace_id: string;
		},
		now: string,
		derivedProvenance: DerivedMemoryProvenance | null,
	): DedupHit | null {
		if (!dedupKey) return null;
		const matchTitle = getMemoryDedupMatchText(title);
		if (!matchTitle) return null;
		const repositoryClause = derivedProvenance ? "AND repository_identity = ?" : "";
		const dedupClause = derivedProvenance
			? "AND dedup_key = ?"
			: "AND (dedup_key = ? OR dedup_key IS NULL)";
		const dedupParams = derivedProvenance
			? [derivedProvenance.repositoryIdentity, dedupKey]
			: [dedupKey];

		const sameSessionRows = this.db
			.prepare(
				`SELECT id, title
					 FROM memory_items
					 WHERE active = 1
				   AND session_id = ?
				   AND kind = ?
				   AND visibility = ?
				   AND workspace_id = ?
				   AND (scope_id = ? OR scope_id IS NULL OR TRIM(scope_id) = '')
				   ${repositoryClause}
				   ${dedupClause}
				 ORDER BY created_at DESC, id DESC`,
			)
			.all(
				sessionId,
				kind,
				provenance.visibility,
				provenance.workspace_id,
				scopeId,
				...dedupParams,
			) as Array<{
			id: number;
			title: string;
		}>;
		for (const row of sameSessionRows) {
			if (getMemoryDedupMatchText(row.title) === matchTitle) {
				return { id: row.id, scope: "same_session" };
			}
		}

		// Cross-session matching is intentionally best-effort. We want to avoid
		// obvious duplicate bursts from adjacent sessions, but we do not enforce
		// a global uniqueness constraint because the time-window heuristic is not
		// strong enough to be a durable identity rule.
		if (this.crossSessionDedupWindowMs <= 0) return null;

		const since = new Date(Date.parse(now) - this.crossSessionDedupWindowMs).toISOString();
		const crossSessionRows = this.db
			.prepare(
				`SELECT id, title
					 FROM memory_items
					 WHERE active = 1
				   AND session_id != ?
				   AND kind = ?
				   AND visibility = ?
				   AND workspace_id = ?
				   AND scope_id = ?
				   AND created_at >= ?
				   ${repositoryClause}
				   ${dedupClause}
				 ORDER BY confidence DESC, created_at DESC, id DESC`,
			)
			.all(
				sessionId,
				kind,
				provenance.visibility,
				provenance.workspace_id,
				scopeId,
				since,
				...dedupParams,
			) as Array<{
			id: number;
			title: string;
		}>;
		for (const row of crossSessionRows) {
			if (getMemoryDedupMatchText(row.title) === matchTitle) {
				return { id: row.id, scope: "cross_session" };
			}
		}
		return null;
	}

	private strengthenDerivedDuplicate(memoryId: number, provenance: DerivedMemoryProvenance): void {
		const memory = this.db
			.prepare("SELECT sensitivity, repository_identity FROM memory_items WHERE id = ?")
			.get(memoryId) as
			| { sensitivity: SensitivityV1; repository_identity: string | null }
			| undefined;
		if (memory?.repository_identity !== provenance.repositoryIdentity) {
			throw new Error("deduplicated memory provenance is invalid");
		}
		const sensitivity = strongestSensitivity(memory.sensitivity, provenance.sensitivity);
		if (sensitivity !== memory.sensitivity) {
			this.db
				.prepare("UPDATE memory_items SET sensitivity = ? WHERE id = ?")
				.run(sensitivity, memoryId);
		}
	}

	private logDedupHit(hit: DedupHit, kind: string): void {
		if (process.env[CODEMEM_DEBUG_ENV] !== "1") return;
		process.stderr.write(
			`[codemem] memory dedup hit scope=${hit.scope} existing_id=${hit.id} kind=${kind}\n`,
		);
	}

	private enqueueVectorWrite(memoryId: number, title: string, bodyText: string): void {
		if (this.db.inTransaction) return;
		let op: Promise<void> | null = null;
		op = storeVectors(this.db, memoryId, title, bodyText)
			.catch(() => {
				// Non-fatal — keep memory writes resilient when embeddings are unavailable
			})
			.finally(() => {
				if (op) this.pendingVectorWrites.delete(op);
			});
		this.pendingVectorWrites.add(op);
	}

	async flushPendingVectorWrites(): Promise<void> {
		if (this.pendingVectorWrites.size === 0) return;
		await Promise.allSettled([...this.pendingVectorWrites]);
	}

	/**
	 * Build the ownership/scope-visibility filter context for SQL filter
	 * builders. Snapshots claimed same-actor peers (and their legacy-sync actor
	 * ids) once so `ownership_scope=mine/theirs` SQL matches
	 * {@link buildOwnershipPredicate}. Shared by core read paths and viewer
	 * routes to keep a single source of truth for "is this mine?".
	 */
	ownershipFilterContext(destinationBoundary?: DestinationBoundaryV1): OwnershipFilterContext {
		const claimedDeviceIds = this.sameActorPeerIds();
		return {
			actorId: this.actorId,
			deviceId: this.deviceId,
			claimedDeviceIds,
			legacyActorIds: claimedDeviceIds.map((peerId) => `legacy-sync:${peerId}`),
			enforceScopeVisibility: true,
			// Resolve the visible scope set once per request so the SQL filter can
			// use the index-eligible `scope_id IN (...)` fast path instead of the
			// per-row EXISTS predicate. Fronts get()/recent()/recentByKinds() and
			// every viewer-server route.
			visibleScopeIds: resolveVisibleScopeIds(this.db, this.deviceId),
			// INVARIANT: no boundary = internal/owner-local processing read (no
			// sensitivity predicate). Every destination surface — viewer routes,
			// daemon RPC read handlers, export — must compile and pass its own
			// boundary explicitly; a new destination caller must never rely on
			// this default.
			destinationBoundary,
		};
	}

	private scopeVisibleFilterContext(
		destinationBoundary?: DestinationBoundaryV1,
	): OwnershipFilterContext {
		return this.ownershipFilterContext(destinationBoundary);
	}

	private readBoundary(
		boundary: DestinationBoundaryV1 | undefined,
		consumer: DestinationConsumerV1,
	): DestinationBoundaryV1 {
		return (
			boundary ??
			compileUntrustedDestinationBoundary({
				consumer,
				configurationFingerprint: CAPTURE_ONLY_DESTINATION_FINGERPRINT,
			})
		);
	}

	// get

	/**
	 * Fetch a single memory item by ID.
	 * Returns null if not found (does not filter by active status).
	 */
	get(memoryId: number, destinationBoundary?: DestinationBoundaryV1): MemoryItemResponse | null {
		const filterResult = buildFilterClausesWithContext(
			null,
			this.scopeVisibleFilterContext(destinationBoundary),
		);
		const whereSql = buildWhereSql(
			["memory_items.id = ?", ...filterResult.clauses],
			[memoryId, ...filterResult.params],
		);
		const row = this.d.get<MemoryItem>(
			sql`SELECT memory_items.* FROM memory_items WHERE ${whereSql}`,
		);
		if (!row) return null;
		return parseMetadata(row);
	}

	// startSession / endSession

	/**
	 * Create a new session row. Returns the session ID.
	 * Matches Python's store.start_session().
	 */
	startSession(opts: {
		cwd?: string;
		project?: string | null;
		gitRemote?: string | null;
		gitBranch?: string | null;
		user?: string;
		toolVersion?: string;
		metadata?: Record<string, unknown>;
	}): number {
		const now = nowIso();
		const rows = this.d
			.insert(schema.sessions)
			.values({
				started_at: now,
				cwd: opts.cwd ?? process.cwd(),
				project: opts.project ?? null,
				git_remote: opts.gitRemote ?? null,
				git_branch: opts.gitBranch ?? null,
				user: opts.user ?? process.env.USER ?? "unknown",
				tool_version: opts.toolVersion ?? "manual",
				metadata_json: toJson(opts.metadata ?? {}),
			})
			.returning({ id: schema.sessions.id })
			.all();
		const id = rows[0]?.id;
		if (id == null) throw new Error("session insert returned no id");
		return id;
	}

	getOrCreateSessionForOpencodeSession(opts: {
		opencodeSessionId: string;
		source?: string;
		cwd?: string;
		project?: string | null;
		metadata?: Record<string, unknown>;
		startedAt?: string | null;
		toolVersion?: string;
		user?: string;
		repositoryIdentity?: string | null;
	}): number {
		const [source, streamId] = this.normalizeStreamIdentity(
			opts.source ?? "opencode",
			opts.opencodeSessionId,
		);
		const existing = this.d
			.select({ session_id: schema.opencodeSessions.session_id })
			.from(schema.opencodeSessions)
			.where(
				and(
					eq(schema.opencodeSessions.source, source),
					eq(schema.opencodeSessions.stream_id, streamId),
				),
			)
			.get();
		if (existing?.session_id != null) {
			if (opts.repositoryIdentity) {
				this.db
					.prepare(
						"UPDATE sessions SET repository_identity = COALESCE(repository_identity, ?) WHERE id = ?",
					)
					.run(validatedRepositoryIdentity(opts.repositoryIdentity), existing.session_id);
			}
			return Number(existing.session_id);
		}

		const startedAt = opts.startedAt ?? nowIso();
		const sessionRows = this.d
			.insert(schema.sessions)
			.values({
				started_at: startedAt,
				cwd: opts.cwd ?? process.cwd(),
				project: opts.project ?? null,
				git_remote: null,
				git_branch: null,
				user: opts.user ?? process.env.USER ?? "unknown",
				tool_version: opts.toolVersion ?? "raw_events",
				metadata_json: toJson(opts.metadata ?? {}),
				repository_identity: validatedRepositoryIdentity(opts.repositoryIdentity),
			})
			.returning({ id: schema.sessions.id })
			.all();
		const sessionId = sessionRows[0]?.id;
		if (sessionId == null) throw new Error("session insert returned no id");

		this.d
			.insert(schema.opencodeSessions)
			.values({
				opencode_session_id: streamId,
				source,
				stream_id: streamId,
				session_id: sessionId,
				created_at: nowIso(),
			})
			.onConflictDoUpdate({
				target: [schema.opencodeSessions.source, schema.opencodeSessions.stream_id],
				set: {
					opencode_session_id: sql`excluded.opencode_session_id`,
					session_id: sql`excluded.session_id`,
				},
			})
			.run();

		return Number(sessionId);
	}

	/**
	 * End a session by recording ended_at.
	 * FIX: merges incoming metadata with existing instead of replacing.
	 * No-op if session doesn't exist.
	 */
	endSession(sessionId: number, metadata?: Record<string, unknown>): void {
		const now = nowIso();
		if (metadata) {
			// Read existing metadata and merge — prevents clobbering earlier fields
			const existing = this.d
				.select({ metadata_json: schema.sessions.metadata_json })
				.from(schema.sessions)
				.where(eq(schema.sessions.id, sessionId))
				.get();
			const merged = { ...fromJson(existing?.metadata_json), ...metadata };
			this.d
				.update(schema.sessions)
				.set({ ended_at: now, metadata_json: toJson(merged) })
				.where(eq(schema.sessions.id, sessionId))
				.run();
		} else {
			this.d
				.update(schema.sessions)
				.set({ ended_at: now })
				.where(eq(schema.sessions.id, sessionId))
				.run();
		}
	}

	// remember

	/**
	 * Create a new memory item. Returns the new memory ID.
	 *
	 * Validates and normalizes the kind. Resolves provenance fields (actor_id,
	 * visibility, workspace_id, trust_state) matching Python's
	 * _resolve_memory_provenance logic.
	 */
	remember(
		sessionId: number,
		kind: string,
		title: string,
		bodyText: string,
		confidence = 0.5,
		tags?: string[],
		metadata?: Record<string, unknown>,
	): number {
		return this.rememberInternal(
			sessionId,
			kind,
			title,
			bodyText,
			confidence,
			tags,
			metadata,
			null,
		);
	}

	rememberTrusted(
		sessionId: number,
		kind: string,
		title: string,
		bodyText: string,
		confidence: number,
		tags: string[] | undefined,
		metadata: Record<string, unknown> | undefined,
		disposition: TrustedMemoryDisposition,
	): number {
		const sensitivity = validatedSensitivity(disposition.sensitivity);
		const repositoryIdentity = validatedRepositoryIdentity(disposition.repositoryIdentity);
		return this.rememberInternal(
			sessionId,
			kind,
			title,
			bodyText,
			confidence,
			tags,
			metadata,
			null,
			{
				// Pair invariant: a restricted row without a repository identity can
				// never satisfy any destination boundary (every restricted predicate
				// requires repository_identity = ?), so it would be write-only and
				// silently absent from exports. Default it to secret — the same
				// normalization the import path applies.
				sensitivity:
					(sensitivity === "local_only" || sensitivity === "private") && repositoryIdentity === null
						? "secret"
						: sensitivity,
				repositoryIdentity,
			},
		);
	}

	private rememberInternal(
		sessionId: number,
		kind: string,
		title: string,
		bodyText: string,
		confidence: number,
		tags: string[] | undefined,
		metadata: Record<string, unknown> | undefined,
		derivedProvenance: DerivedMemoryProvenance | null,
		trustedDisposition: TrustedMemoryDisposition | null = null,
	): number {
		const validKind = validateMemoryKind(kind);
		const now = nowIso();

		// Scan the complete write once in the shared killable worker. On worker
		// failure, preserve only non-content metadata needed to surface degraded
		// delivery without ever writing the unscanned body.
		const scannerOptions = this.scanner.workerOptions();
		const workerDeadlineAtMs = scannerOptions.degraded ? null : prepareRedactionWorkerForScan();
		const scan =
			workerDeadlineAtMs !== null
				? redactValueInWorker(
						{ title, bodyText, tags, metadata: metadata ?? {} },
						scannerOptions.rules ?? [],
						workerDeadlineAtMs,
						scannerOptions.allowlist,
					)
				: { ok: false as const };
		const value = scan.ok ? (scan.value as Record<string, unknown>) : {};
		const safeTitle = typeof value.title === "string" ? value.title : "";
		const safeBody = typeof value.bodyText === "string" ? value.bodyText : "";
		const safeTags = Array.isArray(value.tags)
			? value.tags.filter((tag): tag is string => typeof tag === "string")
			: undefined;
		const tagsText = safeTags ? [...new Set(safeTags)].sort().join(" ") : "";
		const metaPayload: Record<string, unknown> =
			scan.ok &&
			value.metadata &&
			typeof value.metadata === "object" &&
			!Array.isArray(value.metadata)
				? { ...(value.metadata as Record<string, unknown>) }
				: { redaction_degraded: true };
		const dedupKey = derivedProvenance
			? buildDerivedMemoryDedupKey(
					safeTitle,
					derivedProvenance.repositoryIdentity,
					derivedProvenance.dedupSourceIdentity,
				)
			: buildMemoryDedupKey(safeTitle);

		metaPayload.clock_device_id ??= this.deviceId;
		if (derivedProvenance) {
			metaPayload.derived_source = derivedProvenance.source;
			metaPayload.derived_stream_id = derivedProvenance.streamId;
		}
		const importKey = (metaPayload.import_key as string) || randomUUID();
		metaPayload.import_key = importKey;

		// Extract dedicated columns from metadata before they get buried in metadata_json
		const subtitle = typeof metaPayload.subtitle === "string" ? metaPayload.subtitle : null;
		const narrative = typeof metaPayload.narrative === "string" ? metaPayload.narrative : null;
		const facts = Array.isArray(metaPayload.facts) ? metaPayload.facts : null;
		const concepts = Array.isArray(metaPayload.concepts) ? metaPayload.concepts : null;
		const filesRead = Array.isArray(metaPayload.files_read) ? metaPayload.files_read : null;
		const filesModified = Array.isArray(metaPayload.files_modified)
			? metaPayload.files_modified
			: null;
		const promptNumber =
			typeof metaPayload.prompt_number === "number" ? metaPayload.prompt_number : null;
		const userPromptId =
			typeof metaPayload.user_prompt_id === "number" ? metaPayload.user_prompt_id : null;

		// Resolve provenance fields
		const provenance = this.resolveProvenance(metaPayload);
		const scopeId = resolveSessionScopeId(this.db, {
			sessionId,
			workspaceId: provenance.workspace_id,
		});

		// Denormalize the session's project name onto memory_items so that
		// cross-device sync (which does not replicate sessions) can still
		// surface this memory under its real project identity on peers.
		const sessionProject = (this.d
			.select({ project: schema.sessions.project })
			.from(schema.sessions)
			.where(eq(schema.sessions.id, sessionId))
			.get()?.project ?? null) as string | null;

		const existingHit = this.findExistingDuplicateMemory(
			sessionId,
			validKind,
			safeTitle,
			dedupKey,
			scopeId,
			provenance,
			now,
			derivedProvenance,
		);
		if (existingHit != null) {
			if (derivedProvenance) {
				this.strengthenDerivedDuplicate(existingHit.id, derivedProvenance);
			}
			this.logDedupHit(existingHit, validKind);
			return existingHit.id;
		}

		let memoryId: number;
		try {
			memoryId = this.db.transaction(() => {
				const insertedRows = this.d
					.insert(schema.memoryItems)
					.values({
						session_id: sessionId,
						kind: validKind,
						title: safeTitle,
						subtitle,
						body_text: safeBody,
						confidence,
						tags_text: tagsText,
						active: 1,
						created_at: now,
						updated_at: now,
						metadata_json: toJson(metaPayload),
						actor_id: provenance.actor_id,
						actor_display_name: provenance.actor_display_name,
						visibility: provenance.visibility,
						workspace_id: provenance.workspace_id,
						workspace_kind: provenance.workspace_kind,
						origin_device_id: provenance.origin_device_id,
						origin_source: provenance.origin_source,
						trust_state: provenance.trust_state,
						narrative,
						facts: toJsonNullable(facts),
						concepts: toJsonNullable(concepts),
						files_read: toJsonNullable(filesRead),
						files_modified: toJsonNullable(filesModified),
						prompt_number: promptNumber,
						user_prompt_id: userPromptId,
						deleted_at: null,
						rev: 1,
						dedup_key: dedupKey,
						import_key: importKey,
						scope_id: scopeId,
						project: sessionProject,
						...(derivedProvenance
							? {
									sensitivity: derivedProvenance.sensitivity,
									repository_identity: derivedProvenance.repositoryIdentity,
									lineage_id: derivedProvenance.lineageId,
									derivation_key: derivedProvenance.derivationKey,
									source_event_ids_json: toJson(derivedProvenance.sourceEventIds),
									source_spans_json: toJson(derivedProvenance.sourceSpans),
								}
							: trustedDisposition
								? {
										sensitivity: trustedDisposition.sensitivity,
										repository_identity: trustedDisposition.repositoryIdentity,
									}
								: {}),
					})
					.returning({ id: schema.memoryItems.id })
					.all();

				const id = insertedRows[0]?.id;
				if (id == null) throw new Error("memory insert returned no id");

				populateMemoryRefs(this.db, id, filesRead, filesModified, concepts);

				return id;
			})();
		} catch (error) {
			if (!isSameSessionDedupConstraintError(error)) throw error;
			const existingSameSessionHit = this.findExistingDuplicateMemory(
				sessionId,
				validKind,
				safeTitle,
				dedupKey,
				scopeId,
				provenance,
				now,
				derivedProvenance,
			);
			if (existingSameSessionHit != null) {
				if (derivedProvenance) {
					this.strengthenDerivedDuplicate(existingSameSessionHit.id, derivedProvenance);
				}
				this.logDedupHit(existingSameSessionHit, validKind);
				return existingSameSessionHit.id;
			}
			throw error;
		}

		this.enqueueVectorWrite(memoryId, safeTitle, safeBody);

		const detections = scan.ok ? scan.detections : [];
		if (detections.length > 0) {
			this.logSecretRedactions(memoryId, validKind, detections);
		}

		return memoryId;
	}

	/**
	 * Log secret-redaction events for observability. Records the kind and
	 * count only — never the matched value. Gated on CODEMEM_DEBUG so normal
	 * use stays quiet, but the call site always runs so test coverage and
	 * future structured-logging hooks have a single chokepoint to grow into.
	 */
	private logSecretRedactions(memoryId: number, kind: string, detections: ScanDetection[]): void {
		if (process.env[CODEMEM_DEBUG_ENV] !== "1") return;
		const summary = detections.map((d) => `${d.kind}=${d.count}`).join(",");
		process.stderr.write(
			`[codemem] secret_scanner redacted memory_id=${memoryId} kind=${kind} detections=${summary}\n`,
		);
	}

	// provenance resolution

	/**
	 * Resolve provenance fields for a new memory, matching Python's
	 * _resolve_memory_provenance. Uses metadata overrides when present,
	 * falls back to store-level defaults.
	 */
	private resolveProvenance(metadata: Record<string, unknown>): {
		actor_id: string | null;
		actor_display_name: string | null;
		visibility: string;
		workspace_id: string;
		workspace_kind: string;
		origin_device_id: string;
		origin_source: string | null;
		trust_state: string;
	} {
		const clean = (v: unknown): string | null => {
			if (v == null) return null;
			const s = String(v).trim();
			return s.length > 0 ? s : null;
		};

		const actorId = clean(metadata.actor_id) ?? this.actorId;
		const actorDisplayName = clean(metadata.actor_display_name) ?? this.actorDisplayName;

		const explicitWorkspaceKind = clean(metadata.workspace_kind);
		const explicitWorkspaceId = clean(metadata.workspace_id);

		// Visibility defaults to "shared" (matches Python behavior)
		let visibility = clean(metadata.visibility);
		if (!visibility || (visibility !== "private" && visibility !== "shared")) {
			visibility = "shared";
		}

		// Workspace kind derives from visibility
		let workspaceKind = explicitWorkspaceKind ?? "shared";
		if (workspaceKind !== "personal" && workspaceKind !== "shared") {
			workspaceKind = visibility === "shared" ? "shared" : "personal";
		} else if (visibility === "shared") {
			workspaceKind = "shared";
		} else if (visibility === "private") {
			workspaceKind = "personal";
		}

		// Workspace ID with fallback — matches Python's _default_workspace_id
		const workspaceId =
			explicitWorkspaceId ??
			(workspaceKind === "personal" ? `personal:${actorId}` : "shared:default");

		const originDeviceId = clean(metadata.origin_device_id) ?? this.deviceId;
		const originSource = clean(metadata.origin_source) ?? clean(metadata.source) ?? null;
		const trustState = clean(metadata.trust_state) ?? "trusted";

		return {
			actor_id: actorId,
			actor_display_name: actorDisplayName,
			visibility,
			workspace_id: workspaceId,
			workspace_kind: workspaceKind,
			origin_device_id: originDeviceId,
			origin_source: originSource,
			trust_state: trustState,
		};
	}

	// ownership check

	/**
	 * Check if a memory item is owned by this actor/device.
	 * Port of Python's memory_owned_by_self().
	 *
	 * Python checks:
	 * 1. actor_id == self.actor_id → owned
	 * 2. origin_device_id in claimed_same_actor_peers → owned
	 * 3. actor_id in legacy sync actor ids → owned
	 */
	memoryOwnedBySelf(item: OwnershipCandidate): boolean {
		// Always re-reads sync_peers via sameActorPeerIds(). This method
		// authorizes mutating APIs (forgetMemory / setMemoryVisibility),
		// so it must reflect legacy peer claim changes
		// the moment they land — no caching here. Callers that legitimately
		// loop over many memories (search ranking, widening filters) should
		// snapshot once via buildOwnershipPredicate() and reuse the closure.
		return this.buildOwnershipPredicate()(item);
	}

	/**
	 * Snapshot the current ownership state into a fast inline predicate.
	 *
	 * Used to amortize the sync_peers SELECTs across hot loops in the
	 * search ranker. The closure captures the actor + device + claimed-peer
	 * sets at construction time and returns synchronous boolean checks
	 * without touching sqlite. Because the snapshot is frozen, callers
	 * should rebuild the predicate once per request — never share one
	 * across requests where peer claims could change in between.
	 */
	buildOwnershipPredicate(): (item: OwnershipCandidate) => boolean {
		const peerIds = this.sameActorPeerIds();
		const claimedDeviceIds = new Set(peerIds);
		const legacyActorIds = new Set(peerIds.map((peerId) => `legacy-sync:${peerId}`));
		const ownerActorId = this.actorId;
		const ownerDeviceId = this.deviceId;
		return (item) => {
			const rec = item as Record<string, unknown>;
			let meta = (rec.metadata ?? {}) as Record<string, unknown>;
			if (!rec.metadata && typeof rec.metadata_json === "string") {
				meta = fromJson(rec.metadata_json);
			}
			const actorId = cleanStr(rec.actor_id) ?? cleanStr(meta.actor_id);
			if (actorId === ownerActorId) return true;
			const deviceId = cleanStr(rec.origin_device_id) ?? cleanStr(meta.origin_device_id);
			if (deviceId === ownerDeviceId) return true;
			if (deviceId && claimedDeviceIds.has(deviceId)) return true;
			if (actorId && legacyActorIds.has(actorId)) return true;
			return false;
		};
	}

	// forget

	/**
	 * Soft-delete a memory item (set active = 0, record deleted_at).
	 * Updates metadata_json with clock_device_id for replication tracing.
	 * No-op if the memory doesn't exist.
	 */
	forget(memoryId: number): void {
		this.db
			.transaction(() => {
				const row = this.d
					.select({
						rev: schema.memoryItems.rev,
						metadata_json: schema.memoryItems.metadata_json,
						visibility: schema.memoryItems.visibility,
					})
					.from(schema.memoryItems)
					.where(eq(schema.memoryItems.id, memoryId))
					.get();
				if (!row) return;

				const meta = fromJson(row.metadata_json);
				meta.clock_device_id = this.deviceId;

				const now = nowIso();
				const rev = (row.rev ?? 0) + 1;

				this.d
					.update(schema.memoryItems)
					.set({
						active: 0,
						deleted_at: now,
						updated_at: now,
						metadata_json: toJson(meta),
						rev,
					})
					.where(eq(schema.memoryItems.id, memoryId))
					.run();
			})
			.immediate();
	}

	// recent

	/**
	 * Return recent active memories, newest first.
	 * Supports optional filters via buildFilterClauses.
	 */
	recent(
		limit = 10,
		filters?: MemoryFilters | null,
		offset = 0,
		destinationBoundary?: DestinationBoundaryV1,
	): MemoryItemResponse[] {
		const baseClauses = ["memory_items.active = 1"];
		const filterResult = buildFilterClausesWithContext(
			filters,
			this.scopeVisibleFilterContext(destinationBoundary),
		);
		const allClauses = [...baseClauses, ...filterResult.clauses];
		const whereSql = buildWhereSql(allClauses, filterResult.params);

		// Note: joinSessions is set by the project filter (not yet ported).
		// Once project filtering lands, it will trigger the sessions JOIN.
		const fromSql = filterResult.joinSessions
			? sql.raw("memory_items JOIN sessions ON sessions.id = memory_items.session_id")
			: sql.raw("memory_items");

		const rows = this.d.all<MemoryItem>(
			sql`SELECT memory_items.* FROM ${fromSql}
				WHERE ${whereSql}
				ORDER BY created_at DESC, id DESC
				LIMIT ${limit} OFFSET ${Math.max(offset, 0)}`,
		);

		return rows.map((row) => parseMetadata(row));
	}

	// recentByKinds

	/**
	 * Return recent active memories filtered to specific kinds, newest first.
	 */
	recentByKinds(
		kinds: string[],
		limit = 10,
		filters?: MemoryFilters | null,
		offset = 0,
		destinationBoundary?: DestinationBoundaryV1,
	): MemoryItemResponse[] {
		const kindsList = kinds.filter((k) => k.length > 0);
		if (kindsList.length === 0) return [];

		const kindPlaceholders = kindsList.map(() => "?").join(", ");
		const baseClauses = ["memory_items.active = 1", `memory_items.kind IN (${kindPlaceholders})`];
		const filterResult = buildFilterClausesWithContext(
			filters,
			this.scopeVisibleFilterContext(destinationBoundary),
		);
		const allClauses = [...baseClauses, ...filterResult.clauses];
		const params = [...kindsList, ...filterResult.params];
		const whereSql = buildWhereSql(allClauses, params);

		const fromSql = filterResult.joinSessions
			? sql.raw("memory_items JOIN sessions ON sessions.id = memory_items.session_id")
			: sql.raw("memory_items");

		const rows = this.d.all<MemoryItem>(
			sql`SELECT memory_items.* FROM ${fromSql}
				WHERE ${whereSql}
				ORDER BY created_at DESC, id DESC
				LIMIT ${limit} OFFSET ${Math.max(offset, 0)}`,
		);

		return rows.map((row) => parseMetadata(row));
	}

	// usageAggregate

	/**
	 * Neutral, unfiltered token/event aggregate over usage_events, grouped by
	 * event kind. This is the shared SQL aggregate used by both stats() and the
	 * viewer /api/usage route so neither has to scan the (potentially large)
	 * usage_events table into JS to sum it. The COALESCE on tokens_saved matches
	 * the historical stats() semantics (treat NULL saved as 0). When
	 * projectFilter is a non-empty string, rows are restricted to the given
	 * project via the sessions join; otherwise every row is aggregated.
	 * Callers sort the returned rows as needed.
	 */
	usageAggregate(
		projectFilter?: string | null,
		destinationBoundary?: DestinationBoundaryV1,
	): Array<{
		event: string;
		count: number;
		tokens_read: number;
		tokens_written: number;
		tokens_saved: number;
	}> {
		// Two near-identical GROUP BYs kept deliberately separate: the global
		// variant avoids a needless sessions join, and the projections (incl.
		// the tokens_saved double-COALESCE) must stay byte-identical in both.
		const hasProject = typeof projectFilter === "string" && projectFilter.length > 0;
		const visible = destinationBoundary
			? buildFilterClausesWithContext(null, this.scopeVisibleFilterContext(destinationBoundary))
			: null;
		const visibleUsageClause = visible
			? `AND EXISTS (
				SELECT 1 FROM memory_items
				WHERE memory_items.session_id = usage_events.session_id
					AND memory_items.active = 1 AND ${visible.clauses.join(" AND ")}
			 )`
			: "";
		const rows = hasProject
			? (this.db
					.prepare(
						`SELECT usage_events.event AS event,
							COUNT(*) AS count,
							COALESCE(SUM(usage_events.tokens_read), 0) AS tokens_read,
							COALESCE(SUM(usage_events.tokens_written), 0) AS tokens_written,
							COALESCE(SUM(COALESCE(usage_events.tokens_saved, 0)), 0) AS tokens_saved
						 FROM usage_events
						 JOIN sessions ON sessions.id = usage_events.session_id
						 WHERE sessions.project = ? ${visibleUsageClause}
						 GROUP BY usage_events.event`,
					)
					.all(projectFilter, ...(visible?.params ?? [])) as Record<string, unknown>[])
			: (this.db
					.prepare(
						`SELECT event AS event,
							COUNT(*) AS count,
							COALESCE(SUM(tokens_read), 0) AS tokens_read,
							COALESCE(SUM(tokens_written), 0) AS tokens_written,
							COALESCE(SUM(COALESCE(tokens_saved, 0)), 0) AS tokens_saved
						 FROM usage_events
						 WHERE 1 = 1 ${visibleUsageClause}
						 GROUP BY event`,
					)
					.all(...(visible?.params ?? [])) as Record<string, unknown>[]);
		return rows.map((row) => ({
			event: String(row.event),
			count: Number(row.count ?? 0),
			tokens_read: Number(row.tokens_read ?? 0),
			tokens_written: Number(row.tokens_written ?? 0),
			tokens_saved: Number(row.tokens_saved ?? 0),
		}));
	}

	// stats

	/**
	 * Return database statistics matching the Python stats() output shape.
	 */
	stats(destinationBoundary?: DestinationBoundaryV1): StoreStats {
		// biome-ignore lint/suspicious/noExplicitAny: Drizzle table union type is unwieldy
		const countRows = (tbl: any) =>
			this.d.select({ c: sql<number>`COUNT(*)` }).from(tbl).get()?.c ?? 0;
		const visibleFilter = buildFilterClausesWithContext(
			null,
			this.scopeVisibleFilterContext(destinationBoundary),
		);
		const countVisibleMemoryRows = (extraClauses: string[] = []): number => {
			const clauses = [...extraClauses, ...visibleFilter.clauses];
			const row = this.db
				.prepare(`SELECT COUNT(*) AS c FROM memory_items WHERE ${clauses.join(" AND ")}`)
				.get(...visibleFilter.params) as { c: number | null } | undefined;
			return row?.c ?? 0;
		};
		const countVisibleMemorySessions = (): number => {
			const clauses = ["memory_items.active = 1", ...visibleFilter.clauses];
			const row = this.db
				.prepare(
					`SELECT COUNT(DISTINCT memory_items.session_id) AS c
					 FROM memory_items
					 WHERE ${clauses.join(" AND ")}`,
				)
				.get(...visibleFilter.params) as { c: number | null } | undefined;
			return row?.c ?? 0;
		};

		const totalMemories = countVisibleMemoryRows();
		const activeMemories = countVisibleMemoryRows(["memory_items.active = 1"]);
		const sessions = countVisibleMemorySessions();
		const artifacts = destinationBoundary
			? (() => {
					const predicate = memoryDestinationBoundarySql(destinationBoundary, "artifacts");
					// Same gate as the viewer artifact route/count: an artifact counts
					// only when its session has a visible active memory and no
					// restricted active memory — otherwise the aggregate reveals
					// artifacts the route 404s. NOT COALESCE keeps NULL-identity rows
					// (three-valued predicate) on the restricted side.
					const visible = visibleFilter.clauses.join(" AND ");
					return Number(
						(
							this.db
								.prepare(
									`SELECT COUNT(*) AS c FROM artifacts
									 WHERE ${predicate.clause}
									   AND EXISTS (
										SELECT 1 FROM memory_items
										WHERE memory_items.session_id = artifacts.session_id
										  AND memory_items.active = 1 AND ${visible}
									   )
									   AND NOT EXISTS (
										SELECT 1 FROM memory_items
										WHERE memory_items.session_id = artifacts.session_id
										  AND memory_items.active = 1 AND NOT COALESCE((${visible}), 0)
									   )`,
								)
								.get(...predicate.params, ...visibleFilter.params, ...visibleFilter.params) as
								| { c: number }
								| undefined
						)?.c ?? 0,
					);
				})()
			: countRows(schema.artifacts);
		const rawEvents = destinationBoundary
			? (() => {
					const predicate = destinationBoundarySql(destinationBoundary, "raw_events");
					return Number(
						(
							this.db
								.prepare(`SELECT COUNT(*) AS c FROM raw_events WHERE ${predicate.clause}`)
								.get(...predicate.params) as { c: number } | undefined
						)?.c ?? 0,
					);
				})()
			: countRows(schema.rawEvents);

		let vectorCount = 0;
		if (!isEmbeddingDisabled() && tableExists(this.db, "memory_vectors")) {
			try {
				const clauses = ["memory_items.active = 1", ...visibleFilter.clauses];
				const row = this.db
					.prepare(
						`SELECT COUNT(*) AS c
						 FROM memory_vectors
						 JOIN memory_items ON memory_items.id = memory_vectors.memory_id
						 WHERE ${clauses.join(" AND ")}`,
					)
					.get(...visibleFilter.params) as { c: number | null } | undefined;
				vectorCount = row?.c ?? 0;
			} catch {
				vectorCount = 0;
			}
		}
		const vectorCoverage = activeMemories > 0 ? Math.min(1, vectorCount / activeMemories) : 0;

		const tagsFilled = countVisibleMemoryRows([
			"memory_items.active = 1",
			"TRIM(memory_items.tags_text) != ''",
		]);
		const tagsCoverage = activeMemories > 0 ? Math.min(1, tagsFilled / activeMemories) : 0;

		let sizeBytes = 0;
		try {
			sizeBytes = statSync(this.dbPath).size;
		} catch {
			// File may not exist yet or be inaccessible
		}

		// Usage stats. Sort by count DESC to preserve the historical
		// ORDER BY COUNT(*) DESC ordering of this block, with event name as a
		// stable tiebreaker so equal-count rows have a deterministic order.
		const usageEvents = this.usageAggregate(null, destinationBoundary).sort(
			(a, b) => b.count - a.count || a.event.localeCompare(b.event),
		);

		const totalEvents = usageEvents.reduce((s, e) => s + e.count, 0);
		const totalTokensRead = usageEvents.reduce((s, e) => s + e.tokens_read, 0);
		const totalTokensWritten = usageEvents.reduce((s, e) => s + e.tokens_written, 0);
		const totalTokensSaved = usageEvents.reduce((s, e) => s + e.tokens_saved, 0);

		return {
			identity: {
				device_id: this.deviceId,
				actor_id: this.actorId,
				actor_display_name: this.actorDisplayName,
			},
			database: {
				path: this.dbPath,
				size_bytes: sizeBytes,
				sessions,
				memory_items: totalMemories,
				active_memory_items: activeMemories,
				artifacts,
				vector_rows: vectorCount,
				vector_coverage: vectorCoverage,
				tags_filled: tagsFilled,
				tags_coverage: tagsCoverage,
				raw_events: rawEvents,
			},
			usage: {
				events: usageEvents,
				totals: {
					events: totalEvents,
					tokens_read: totalTokensRead,
					tokens_written: totalTokensWritten,
					tokens_saved: totalTokensSaved,
				},
			},
		};
	}

	// updateMemoryVisibility

	/**
	 * Update the visibility of an active memory item.
	 * Throws if visibility is invalid, memory not found, memory is inactive,
	 * or memory is not owned by this device/actor.
	 */
	updateMemoryVisibility(memoryId: number, visibility: string): MemoryItemResponse {
		const cleaned = visibility.trim();
		if (cleaned !== "private" && cleaned !== "shared") {
			throw new Error("visibility must be private or shared");
		}

		return this.db
			.transaction(() => {
				const row = this.d
					.select()
					.from(schema.memoryItems)
					.where(and(eq(schema.memoryItems.id, memoryId), eq(schema.memoryItems.active, 1)))
					.get() as MemoryItem | undefined;
				if (!row) {
					throw new Error("memory not found");
				}

				if (!this.memoryOwnedBySelf(row)) {
					throw new Error("memory not owned by this device");
				}

				const rowActorId = cleanStr(row.actor_id) ?? this.actorId;
				const workspaceKind = cleaned === "shared" ? "shared" : "personal";
				const workspaceId =
					cleaned === "shared" && row.workspace_id?.startsWith("shared:")
						? row.workspace_id
						: workspaceKind === "personal"
							? `personal:${rowActorId}`
							: "shared:default";

				const meta = fromJson(row.metadata_json);
				meta.visibility = cleaned;
				meta.workspace_kind = workspaceKind;
				meta.workspace_id = workspaceId;
				meta.clock_device_id = this.deviceId;

				const now = nowIso();
				const rev = (row.rev ?? 0) + 1;

				this.d
					.update(schema.memoryItems)
					.set({
						visibility: cleaned,
						workspace_kind: workspaceKind,
						workspace_id: workspaceId,
						updated_at: now,
						metadata_json: toJson(meta),
						rev,
					})
					.where(eq(schema.memoryItems.id, memoryId))
					.run();

				// Internal readback: the raw row was already fetched and ownership
				// verified above, so this must not route through the fail-closed
				// destination gate in get().
				const updated = this.d
					.select()
					.from(schema.memoryItems)
					.where(eq(schema.memoryItems.id, memoryId))
					.get() as MemoryItem | undefined;
				if (!updated) {
					throw new Error("memory not found after update");
				}
				return parseMetadata(updated);
			})
			.immediate();
	}

	// moveMemoryProject

	/**
	 * Reassign a memory to a different project by mutating its parent
	 * session's project column. Because project attribution is derived
	 * from sessions.project via JOIN, every memory that shares the
	 * session moves with it — the caller is responsible for warning the
	 * user when the session holds more than one memory.
	 *
	 * Local-only mutation: the sessions table is not currently part of
	 * the replication stream, so the move is not propagated to peers.
	 *
	 * Throws if the memory is not found, inactive, or not owned by this
	 * device. Trims the project argument and rejects empty values.
	 */
	moveMemoryProject(
		memoryId: number,
		project: string,
	): { session_id: number; project: string; moved_memory_count: number } {
		const cleanedProject = project.trim();
		if (!cleanedProject) {
			throw new Error("project must be a non-empty string");
		}

		return this.db
			.transaction(() => {
				const row = this.d
					.select()
					.from(schema.memoryItems)
					.where(and(eq(schema.memoryItems.id, memoryId), eq(schema.memoryItems.active, 1)))
					.get() as MemoryItem | undefined;
				if (!row) {
					throw new Error("memory not found");
				}
				if (!this.memoryOwnedBySelf(row)) {
					throw new Error("memory not owned by this device");
				}

				const sessionId = row.session_id;
				this.d
					.update(schema.sessions)
					.set({ project: cleanedProject })
					.where(eq(schema.sessions.id, sessionId))
					.run();

				const countRow = this.d
					.select({ n: sql<number>`count(*)` })
					.from(schema.memoryItems)
					.where(
						and(eq(schema.memoryItems.session_id, sessionId), eq(schema.memoryItems.active, 1)),
					)
					.get() as { n: number } | undefined;

				return {
					session_id: sessionId,
					project: cleanedProject,
					moved_memory_count: Number(countRow?.n ?? 1),
				};
			})
			.immediate();
	}

	// search

	/**
	 * Full-text search for memories using FTS5.
	 *
	 * Delegates to search.ts to keep the search logic decoupled.
	 * Results are ranked by BM25 score, recency, and kind bonus.
	 */
	search(
		query: string,
		limit = 10,
		filters?: MemoryFilters,
		destinationBoundary?: DestinationBoundaryV1,
	): MemoryResult[] {
		return searchFn(this, query, limit, filters, destinationBoundary);
	}

	// timeline

	/**
	 * Return a chronological window of memories around an anchor.
	 *
	 * Finds an anchor by memoryId or query, then fetches neighbors
	 * in the same session. Delegates to search.ts.
	 */
	timeline(
		query?: string | null,
		memoryId?: number | null,
		depthBefore = 3,
		depthAfter = 3,
		filters?: MemoryFilters | null,
		destinationBoundary?: DestinationBoundaryV1,
	): TimelineItemResponse[] {
		return timelineFn(this, query, memoryId, depthBefore, depthAfter, filters, destinationBoundary);
	}

	// explain

	/**
	 * Explain search results with scoring breakdown.
	 *
	 * Returns detailed scoring components for each result, merging
	 * query-based and ID-based lookups. Delegates to search.ts.
	 */
	explain(
		query?: string | null,
		ids?: unknown[] | null,
		limit = 10,
		filters?: MemoryFilters | null,
		options?: ExplainOptions,
		destinationBoundary?: DestinationBoundaryV1,
	): ExplainResponse {
		return explainFn(this, query, ids, limit, filters, options, destinationBoundary);
	}

	// buildMemoryPack

	/**
	 * Build a formatted memory pack from search results.
	 *
	 * Categorizes memories into summary/timeline/observations sections,
	 * with optional token budgeting. Delegates to pack.ts.
	 */
	buildMemoryPack(
		context: string,
		limit?: number,
		tokenBudget?: number | null,
		filters?: MemoryFilters,
		destinationBoundary?: DestinationBoundaryV1,
	): PackResponse {
		return buildMemoryPack(
			this,
			context,
			limit,
			tokenBudget ?? null,
			filters,
			undefined,
			undefined,
			this.readBoundary(destinationBoundary, "daemon_pack"),
		);
	}

	buildMemoryPackTrace(
		context: string,
		limit?: number,
		tokenBudget?: number | null,
		filters?: MemoryFilters,
		destinationBoundary?: DestinationBoundaryV1,
	): PackTrace {
		return buildMemoryPackTrace(
			this,
			context,
			limit,
			tokenBudget ?? null,
			filters,
			undefined,
			undefined,
			this.readBoundary(destinationBoundary, "daemon_pack"),
		);
	}

	buildMemoryPackWithTrace(
		context: string,
		limit?: number,
		tokenBudget?: number | null,
		filters?: MemoryFilters,
		renderOptions?: PackRenderOptions,
		destinationBoundary?: DestinationBoundaryV1,
	): PackArtifacts {
		return buildMemoryPackWithTrace(
			this,
			context,
			limit,
			tokenBudget ?? null,
			filters,
			undefined,
			renderOptions,
			this.readBoundary(destinationBoundary, "daemon_pack"),
		);
	}

	/**
	 * Build a memory pack with semantic candidate merging.
	 *
	 * Async version that runs vector KNN search via sqlite-vec and merges
	 * semantic candidates with FTS results.  Falls back to FTS-only when
	 * embeddings are disabled or unavailable.
	 */
	async buildMemoryPackAsync(
		context: string,
		limit?: number,
		tokenBudget?: number | null,
		filters?: MemoryFilters,
		renderOptions?: PackRenderOptions,
		destinationBoundary?: DestinationBoundaryV1,
	): Promise<PackResponse> {
		return buildMemoryPackAsync(
			this,
			context,
			limit,
			tokenBudget ?? null,
			filters,
			renderOptions,
			this.readBoundary(destinationBoundary, "daemon_pack"),
		);
	}

	async buildMemoryPackWithTraceAsync(
		context: string,
		limit?: number,
		tokenBudget?: number | null,
		filters?: MemoryFilters,
		renderOptions?: PackRenderOptions,
		destinationBoundary?: DestinationBoundaryV1,
	): Promise<PackArtifacts> {
		return buildMemoryPackWithTraceAsync(
			this,
			context,
			limit,
			tokenBudget ?? null,
			filters,
			renderOptions,
			this.readBoundary(destinationBoundary, "daemon_pack"),
		);
	}

	async buildMemoryPackTraceAsync(
		context: string,
		limit?: number,
		tokenBudget?: number | null,
		filters?: MemoryFilters,
		renderOptions?: PackRenderOptions,
		destinationBoundary?: DestinationBoundaryV1,
	): Promise<PackTrace> {
		return buildMemoryPackTraceAsync(
			this,
			context,
			limit,
			tokenBudget ?? null,
			filters,
			renderOptions,
			this.readBoundary(destinationBoundary, "daemon_pack"),
		);
	}

	// Raw event helpers

	/**
	 * Normalize source/streamId to match Python's _normalize_stream_identity().
	 * Trims whitespace, lowercases source, defaults to "opencode".
	 */
	private normalizeStreamIdentity(source: string, streamId: string): [string, string] {
		const s = source.trim().toLowerCase() || "opencode";
		const sid = streamId.trim();
		if (!sid) throw new Error("stream_id is required");
		return [s, sid];
	}

	// Raw event query methods (ports from codemem/store/raw_events.py)

	/**
	 * Find sessions that have unflushed events and have been idle long enough.
	 * Port of raw_event_sessions_pending_idle_flush().
	 */
	rawEventSessionsPendingIdleFlush(
		idleBeforeTsWallMs: number,
		limit = 25,
	): { source: string; streamId: string }[] {
		const maxEvents = this.d
			.select({
				source: schema.rawEvents.source,
				stream_id: schema.rawEvents.stream_id,
				max_seq: sql<number>`MAX(${schema.rawEvents.event_seq})`.as("max_seq"),
			})
			.from(schema.rawEvents)
			.groupBy(schema.rawEvents.source, schema.rawEvents.stream_id)
			.as("max_events");

		const rows = this.d
			.select({
				source: schema.rawEventSessions.source,
				stream_id: schema.rawEventSessions.stream_id,
			})
			.from(schema.rawEventSessions)
			.innerJoin(
				maxEvents,
				and(
					eq(maxEvents.source, schema.rawEventSessions.source),
					eq(maxEvents.stream_id, schema.rawEventSessions.stream_id),
				),
			)
			.where(
				and(
					isNotNull(schema.rawEventSessions.last_seen_ts_wall_ms),
					lte(schema.rawEventSessions.last_seen_ts_wall_ms, idleBeforeTsWallMs),
					gt(maxEvents.max_seq, schema.rawEventSessions.last_flushed_event_seq),
				),
			)
			.orderBy(schema.rawEventSessions.last_seen_ts_wall_ms)
			.limit(limit)
			.all();

		return rows
			.filter((row) => row.stream_id)
			.map((row) => ({
				source: String(row.source ?? "opencode"),
				streamId: String(row.stream_id ?? ""),
			}));
	}

	/**
	 * Find sessions that have pending/failed flush batches with unflushed events.
	 * Port of raw_event_sessions_with_pending_queue().
	 */
	rawEventSessionsWithPendingQueue(limit = 25): { source: string; streamId: string }[] {
		const pendingBatches = this.d
			.select({
				source: schema.rawEventFlushBatches.source,
				stream_id: schema.rawEventFlushBatches.stream_id,
				has_advanced_legacy: sql<number>`MAX(CASE
					WHEN ${sql.raw(ADVANCED_LEGACY_RECOVERY_PREDICATE)}
					THEN 1 ELSE 0 END)`.as("has_advanced_legacy"),
				oldest_pending_update: sql<string>`MIN(${schema.rawEventFlushBatches.updated_at})`.as(
					"oldest_pending_update",
				),
			})
			.from(schema.rawEventFlushBatches)
			.where(inArray(schema.rawEventFlushBatches.status, ["queued", "failed"]))
			.groupBy(schema.rawEventFlushBatches.source, schema.rawEventFlushBatches.stream_id)
			.as("pending_batches");

		const maxEvents = this.d
			.select({
				source: schema.rawEvents.source,
				stream_id: schema.rawEvents.stream_id,
				max_seq: sql<number>`MAX(${schema.rawEvents.event_seq})`.as("max_seq"),
			})
			.from(schema.rawEvents)
			.groupBy(schema.rawEvents.source, schema.rawEvents.stream_id)
			.as("max_events");

		const rows = this.d
			.select({ source: pendingBatches.source, stream_id: pendingBatches.stream_id })
			.from(pendingBatches)
			.innerJoin(
				maxEvents,
				and(
					eq(maxEvents.source, pendingBatches.source),
					eq(maxEvents.stream_id, pendingBatches.stream_id),
				),
			)
			.leftJoin(
				schema.rawEventSessions,
				and(
					eq(schema.rawEventSessions.source, pendingBatches.source),
					eq(schema.rawEventSessions.stream_id, pendingBatches.stream_id),
				),
			)
			.where(
				or(
					gt(
						maxEvents.max_seq,
						sql`COALESCE(${schema.rawEventSessions.last_flushed_event_seq}, -1)`,
					),
					eq(pendingBatches.has_advanced_legacy, 1),
				),
			)
			.orderBy(sql`${pendingBatches.oldest_pending_update}`)
			.limit(limit)
			.all();

		return rows
			.filter((row) => row.stream_id)
			.map((row) => ({
				source: String(row.source ?? "opencode"),
				streamId: String(row.stream_id ?? ""),
			}));
	}

	/**
	 * Delete raw events older than max_age_ms. Returns count of deleted raw_events rows.
	 * Port of purge_raw_events() + purge_raw_events_before().
	 */
	purgeRawEvents(maxAgeMs: number): number {
		if (maxAgeMs <= 0) return 0;
		const nowMs = Date.now();
		const cutoffTsWallMs = nowMs - maxAgeMs;
		const cutoffIso = new Date(cutoffTsWallMs).toISOString();

		return this.db.transaction((): number => {
			this.d
				.delete(schema.rawEventIngestSamples)
				.where(sql`${schema.rawEventIngestSamples.created_at} < ${cutoffIso}`)
				.run();
			const result = this.db
				.prepare(
					`DELETE FROM raw_events
					 WHERE ${RAW_EVENT_PURGE_PREDICATE}`,
				)
				.run(cutoffTsWallMs);
			return Number(result.changes ?? 0);
		})();
	}

	/**
	 * Report raw_events storage usage and how many rows fall before an age cutoff.
	 *
	 * Age-based only (mirrors planReplicationOpsAgePrune / estimateReplicationOps
	 * style): counts total rows, estimates the on-disk bytes for the raw_events
	 * table and its indexes, and counts rows with ts_wall_ms older than the
	 * cutoff implied by maxAgeMs. Used by the prune-raw-events dry-run report.
	 */
	rawEventsRetentionStatus(maxAgeMs: number): {
		total_rows: number;
		estimated_bytes: number;
		candidate_rows: number;
		cutoff_ts_wall_ms: number | null;
	} {
		const totalRow = this.d.select({ total: sql<number>`COUNT(*)` }).from(schema.rawEvents).get();
		const totalRows = Number(totalRow?.total ?? 0);

		let candidateRows = 0;
		let cutoffTsWallMs: number | null = null;
		if (maxAgeMs > 0) {
			cutoffTsWallMs = Date.now() - maxAgeMs;
			const candidateRow = this.db
				.prepare(
					`SELECT COUNT(*) AS total FROM raw_events
					 WHERE ${RAW_EVENT_PURGE_PREDICATE}`,
				)
				.get(cutoffTsWallMs) as { total: number } | undefined;
			candidateRows = Number(candidateRow?.total ?? 0);
		}

		return {
			total_rows: totalRows,
			estimated_bytes: this.estimateRawEventsBytes(),
			candidate_rows: candidateRows,
			cutoff_ts_wall_ms: cutoffTsWallMs,
		};
	}

	private estimateRawEventsBytes(): number {
		try {
			const row = this.db
				.prepare(
					`SELECT COALESCE(SUM(pgsize), 0) AS total_bytes
					 FROM dbstat
					 WHERE name = 'raw_events'
					    OR name LIKE 'idx_raw_events_%'
					    OR name LIKE 'sqlite_autoindex_raw_events_%'`,
				)
				.get() as { total_bytes?: number | string | null } | undefined;
			const total = Number(row?.total_bytes ?? 0);
			return Number.isFinite(total) && total >= 0 ? total : 0;
		} catch {
			return 0;
		}
	}

	/**
	 * Mark stuck flush batches (started/running/pending/claimed) as failed.
	 * Port of mark_stuck_raw_event_batches_as_error().
	 */
	markStuckRawEventBatchesAsError(olderThanIso: string, limit = 100): number {
		return this.recoverStuckRawEventFlushJobs(olderThanIso, limit);
	}

	// Raw event per-session methods (ports for flush pipeline)

	/**
	 * Get session metadata (cwd, project, started_at, etc.) for a raw event stream.
	 * Port of raw_event_session_meta().
	 */
	rawEventSessionMeta(opencodeSessionId: string, source = "opencode"): Record<string, unknown> {
		const [s, sid] = this.normalizeStreamIdentity(source, opencodeSessionId);
		const row = this.d
			.select({
				cwd: schema.rawEventSessions.cwd,
				project: schema.rawEventSessions.project,
				started_at: schema.rawEventSessions.started_at,
				last_seen_ts_wall_ms: schema.rawEventSessions.last_seen_ts_wall_ms,
				last_received_event_seq: schema.rawEventSessions.last_received_event_seq,
				last_flushed_event_seq: schema.rawEventSessions.last_flushed_event_seq,
			})
			.from(schema.rawEventSessions)
			.where(and(eq(schema.rawEventSessions.source, s), eq(schema.rawEventSessions.stream_id, sid)))
			.get();
		if (!row) return {};
		return {
			cwd: row.cwd,
			project: row.project,
			started_at: row.started_at,
			last_seen_ts_wall_ms: row.last_seen_ts_wall_ms,
			last_received_event_seq: row.last_received_event_seq,
			last_flushed_event_seq: row.last_flushed_event_seq,
		};
	}

	/**
	 * Get the last flushed event_seq for a session. Returns -1 if no state.
	 * Port of raw_event_flush_state().
	 */
	rawEventFlushState(opencodeSessionId: string, source = "opencode"): number {
		const [s, sid] = this.normalizeStreamIdentity(source, opencodeSessionId);
		const row = this.d
			.select({ last_flushed_event_seq: schema.rawEventSessions.last_flushed_event_seq })
			.from(schema.rawEventSessions)
			.where(and(eq(schema.rawEventSessions.source, s), eq(schema.rawEventSessions.stream_id, sid)))
			.get();
		if (!row) return -1;
		return Number(row.last_flushed_event_seq);
	}

	/**
	 * Get raw events after a given event_seq, ordered by event_seq ASC.
	 * Returns enriched event objects with type, timestamps, event_seq, event_id.
	 * Port of raw_events_since_by_seq().
	 */
	rawEventsSinceBySeq(
		opencodeSessionId: string,
		source = "opencode",
		afterEventSeq = -1,
		limit?: number | null,
	): Record<string, unknown>[] {
		const [s, sid] = this.normalizeStreamIdentity(source, opencodeSessionId);
		const baseQuery = this.d
			.select({
				event_seq: schema.rawEvents.event_seq,
				event_type: schema.rawEvents.event_type,
				ts_wall_ms: schema.rawEvents.ts_wall_ms,
				ts_mono_ms: schema.rawEvents.ts_mono_ms,
				payload_json: schema.rawEvents.payload_json,
				event_id: schema.rawEvents.event_id,
			})
			.from(schema.rawEvents)
			.where(
				and(
					eq(schema.rawEvents.source, s),
					eq(schema.rawEvents.stream_id, sid),
					gt(schema.rawEvents.event_seq, afterEventSeq),
				),
			)
			.orderBy(schema.rawEvents.event_seq);

		const rows = limit != null && limit > 0 ? baseQuery.limit(limit).all() : baseQuery.all();

		return rows.map((row) => {
			const payload = fromJson(row.payload_json) as Record<string, unknown>;
			// Use || (not ??) to match Python's `or` semantics — empty string falls through
			payload.type = payload.type || row.event_type;
			payload.timestamp_wall_ms = row.ts_wall_ms;
			payload.timestamp_mono_ms = row.ts_mono_ms;
			payload.event_seq = row.event_seq;
			payload.event_id = row.event_id;
			return payload;
		});
	}

	admitRawEventFlushJob(input: {
		source: string;
		streamId: string;
		manifestFingerprint: string;
		providerFingerprint: string;
	}): RawEventJobAdmission {
		const [source, streamId] = this.normalizeStreamIdentity(input.source, input.streamId);
		const manifestFingerprint = requiredFingerprint(
			input.manifestFingerprint,
			"admission_manifest_fingerprint",
		);
		const providerFingerprint = requiredFingerprint(
			input.providerFingerprint,
			"admission_provider_fingerprint",
		);
		return this.db.transaction((): RawEventJobAdmission => {
			const advancedLegacy = this.db
				.prepare(
					`SELECT id, start_event_seq, end_event_seq
					 FROM raw_event_flush_batches
					 WHERE source = ? AND stream_id = ?
					   AND ${ADVANCED_LEGACY_RECOVERY_PREDICATE}
					 ORDER BY id LIMIT 1`,
				)
				.get(source, streamId) as
				| { id: number; start_event_seq: number; end_event_seq: number }
				| undefined;
			if (advancedLegacy) {
				if (
					!hasExactRawEventSourceRange(this.db, {
						source,
						streamId,
						startEventSeq: Number(advancedLegacy.start_event_seq),
						endEventSeq: Number(advancedLegacy.end_event_seq),
					})
				) {
					return { status: "source_gap", reason: "source_gap" };
				}
				return {
					status: "existing",
					jobId: Number(advancedLegacy.id),
					startEventSeq: Number(advancedLegacy.start_event_seq),
					endEventSeq: Number(advancedLegacy.end_event_seq),
				};
			}
			const session = this.db
				.prepare(
					"SELECT last_flushed_event_seq, last_received_event_seq FROM raw_event_sessions WHERE source = ? AND stream_id = ?",
				)
				.get(source, streamId) as
				| { last_flushed_event_seq: number; last_received_event_seq: number }
				| undefined;
			if (!session) return { status: "no_events" };
			const nextSequence = Number(session.last_flushed_event_seq) + 1;
			const existing = this.db
				.prepare(
					`SELECT id, start_event_seq, end_event_seq, status, attempt_count, retry_limit
					 FROM raw_event_flush_batches
					 WHERE source = ? AND stream_id = ? AND start_event_seq = ?
					   AND status IN ('queued','processing','failed','retry_exhausted')
					 ORDER BY id LIMIT 1`,
				)
				.get(source, streamId, nextSequence) as
				| {
						id: number;
						start_event_seq: number;
						end_event_seq: number;
						status: string;
						attempt_count: number;
						retry_limit: number;
				  }
				| undefined;
			if (existing) {
				if (
					!hasExactRawEventSourceRange(this.db, {
						source,
						streamId,
						startEventSeq: Number(existing.start_event_seq),
						endEventSeq: Number(existing.end_event_seq),
					})
				) {
					return { status: "source_gap", reason: "source_gap" };
				}
				if (existing.status === "failed" && existing.attempt_count < existing.retry_limit) {
					this.db
						.prepare(
							"UPDATE raw_event_flush_batches SET status = 'queued', updated_at = ? WHERE id = ? AND status = 'failed'",
						)
						.run(nowIso(), existing.id);
				}
				return {
					status: "existing",
					jobId: Number(existing.id),
					startEventSeq: Number(existing.start_event_seq),
					endEventSeq: Number(existing.end_event_seq),
				};
			}
			const capacity = this.db
				.prepare(
					"SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE status IN ('queued','processing','failed','retry_exhausted')",
				)
				.get() as { count: number };
			if (Number(capacity.count) >= PROCESSING_JOB_CAPACITY) {
				return { status: "capacity", reason: "wait_for_capacity" };
			}
			const sourceRange = inspectRawEventSourceRange(this.db, {
				source,
				streamId,
				lastFlushedEventSeq: Number(session.last_flushed_event_seq),
				lastReceivedEventSeq: Number(session.last_received_event_seq),
			});
			if (sourceRange.status === "empty") return { status: "no_events" };
			if (sourceRange.status === "source_gap") {
				return { status: "source_gap", reason: "source_gap" };
			}
			const eventCount = sourceRange.eventCount;
			const endEventSeq = nextSequence + eventCount - 1;
			const now = nowIso();
			const inserted = this.db
				.prepare(
					`INSERT INTO raw_event_flush_batches(
						source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
						extractor_version, status, admission_manifest_fingerprint,
						admission_provider_fingerprint, attempt_count, claim_generation,
						resume_grant_state, last_resume_sequence, completion_disposition,
						legacy_recovery_state, frontier_already_advanced, created_at, updated_at
					 ) VALUES (?, ?, ?, ?, ?, 'raw_events_v1', 'queued', ?, ?, 0, 0,
						'none', 0, 'none', 'not_legacy', 0, ?, ?)`,
				)
				.run(
					source,
					streamId,
					streamId,
					nextSequence,
					endEventSeq,
					manifestFingerprint,
					providerFingerprint,
					now,
					now,
				);
			return {
				status: "admitted",
				jobId: Number(inserted.lastInsertRowid),
				startEventSeq: nextSequence,
				endEventSeq,
			};
		})();
	}

	private loadBoundRawEventSources(input: {
		source: string;
		streamId: string;
		startEventSeq: number;
		endEventSeq: number;
	}): BoundRawEventSource[] {
		const rows = this.db
			.prepare(
				`SELECT event_id, event_seq, event_type, ts_wall_ms, ts_mono_ms, payload_json,
					sensitivity, repository_identity, capture_state, safe_error_code,
					payload_digest_version, payload_digest
				 FROM raw_events
				 WHERE source = ? AND stream_id = ? AND event_seq BETWEEN ? AND ?
				 ORDER BY event_seq`,
			)
			.all(input.source, input.streamId, input.startEventSeq, input.endEventSeq) as Array<{
			event_id: string;
			event_seq: number;
			event_type: string;
			ts_wall_ms: number | null;
			ts_mono_ms: number | null;
			payload_json: string;
			sensitivity: SensitivityV1;
			repository_identity: string | null;
			capture_state: RawEventCaptureState;
			safe_error_code: string | null;
			payload_digest_version: "event-payload-digest-v1";
			payload_digest: string;
		}>;
		return rows.map((row) => ({
			eventId: row.event_id,
			eventSeq: Number(row.event_seq),
			eventType: row.event_type,
			tsWallMs: row.ts_wall_ms,
			tsMonoMs: row.ts_mono_ms,
			payloadJson: row.payload_json,
			sensitivity: row.sensitivity,
			repositoryIdentity: row.repository_identity,
			captureState: row.capture_state,
			safeErrorCode: row.safe_error_code,
			payloadDigestVersion: row.payload_digest_version,
			payloadDigest: row.payload_digest,
		}));
	}

	private projectBoundRawEventSources(
		boundary: DestinationBoundaryV1,
		allSources: readonly BoundRawEventSource[],
	): BoundRawEventSource[] {
		const acceptedRepositories = new Set(
			allSources
				.filter((source) => source.captureState === "accepted")
				.map((source) => source.repositoryIdentity),
		);
		if (
			boundary.executionLocation === "local" &&
			(boundary.repositoryIdentity === null ||
				acceptedRepositories.size !== 1 ||
				!acceptedRepositories.has(boundary.repositoryIdentity))
		) {
			return [];
		}
		return allSources.filter((source) =>
			isDestinationEligible(boundary, {
				sensitivity: source.sensitivity,
				repositoryIdentity: source.repositoryIdentity,
				captureState: source.captureState,
			}),
		);
	}

	rawEventFlushJobRepositoryIdentity(jobId: number): string | null {
		if (!Number.isInteger(jobId) || jobId <= 0) throw new Error("jobId is invalid");
		const batch = this.db
			.prepare(
				`SELECT source, stream_id, start_event_seq, end_event_seq
				 FROM raw_event_flush_batches WHERE id = ?`,
			)
			.get(jobId) as
			| { source: string; stream_id: string; start_event_seq: number; end_event_seq: number }
			| undefined;
		if (!batch) return null;
		const repositories = new Set(
			this.loadBoundRawEventSources({
				source: batch.source,
				streamId: batch.stream_id,
				startEventSeq: Number(batch.start_event_seq),
				endEventSeq: Number(batch.end_event_seq),
			})
				.filter((source) => source.captureState === "accepted")
				.map((source) => source.repositoryIdentity),
		);
		return repositories.size === 1 ? ([...repositories][0] ?? null) : null;
	}

	claimRawEventFlushJob(input: {
		jobId: number;
		manifestFingerprint: string;
		providerFingerprint: string;
		maxMemoryItemsPerDerivation?: number;
		manifest?: EffectiveCapabilityManifestV1;
		boundary?: DestinationBoundaryV1;
	}): RawEventJobClaim | null {
		if (!Number.isInteger(input.jobId) || input.jobId <= 0) throw new Error("jobId is invalid");
		const manifestFingerprint = requiredFingerprint(
			input.manifestFingerprint,
			"attempt_manifest_fingerprint",
		);
		const providerFingerprint = requiredFingerprint(
			input.providerFingerprint,
			"attempt_provider_fingerprint",
		);
		if (!input.boundary) throw new Error("destination boundary is required");
		const boundary = input.boundary;
		const boundaryFingerprint = destinationBoundaryFingerprint(boundary);
		const manifest = input.manifest ? validateCapabilityManifest(input.manifest) : null;
		if (
			manifest &&
			(manifest.configurationFingerprint !== manifestFingerprint ||
				manifest.summaryProvider.providerFingerprint !== providerFingerprint)
		) {
			throw new Error("claim manifest identity is invalid");
		}
		if (
			boundary.consumer !== "summary_provider" ||
			boundary.configurationFingerprint !== manifestFingerprint ||
			boundary.providerFingerprint !== providerFingerprint ||
			(manifest !== null &&
				(boundary.executionLocation !== manifest.summaryProvider.executionLocation ||
					boundary.targetModel !== manifest.summaryProvider.modelId))
		) {
			throw new Error("claim destination boundary is invalid");
		}
		const maxMemoryItemsPerDerivation =
			input.maxMemoryItemsPerDerivation ??
			manifest?.resourceProfile.maxMemoryItemsPerDerivation ??
			16;
		if (
			!Number.isInteger(maxMemoryItemsPerDerivation) ||
			(maxMemoryItemsPerDerivation !== 16 && maxMemoryItemsPerDerivation !== 17) ||
			(manifest !== null &&
				maxMemoryItemsPerDerivation !== manifest.resourceProfile.maxMemoryItemsPerDerivation) ||
			(maxMemoryItemsPerDerivation === 17 && manifest?.resourceProfile.version !== 2)
		) {
			throw new Error("maxMemoryItemsPerDerivation is invalid");
		}
		return this.db.transaction((): RawEventJobClaim | null => {
			const active = this.db
				.prepare(
					"SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE status = 'processing'",
				)
				.get() as { count: number };
			if (Number(active.count) >= PROCESSING_JOB_CONCURRENCY) return null;
			const row = this.db
				.prepare(
					`SELECT batch.id, batch.source, batch.stream_id, batch.start_event_seq,
						batch.end_event_seq, batch.status, batch.retry_limit, batch.attempt_count,
						batch.claim_generation, batch.resume_grant_state,
						batch.attempt_manifest_fingerprint, batch.attempt_provider_fingerprint,
						batch.attempt_max_memory_items, signal.target_role AS resume_target_role,
						signal.target_provider_fingerprint AS resume_target_provider_fingerprint,
						signal.target_manifest_fingerprint AS resume_target_manifest_fingerprint,
						signal.kind AS resume_signal_kind, signal.disposition AS resume_signal_disposition
					 FROM raw_event_flush_batches AS batch
					 LEFT JOIN processing_resume_signals AS signal
						ON signal.job_id = batch.id AND signal.grant_id = batch.resume_grant_id
						AND signal.disposition = 'accepted'
					 WHERE batch.id = ?`,
				)
				.get(input.jobId) as
				| {
						id: number;
						source: string;
						stream_id: string;
						start_event_seq: number;
						end_event_seq: number;
						status: string;
						retry_limit: number;
						attempt_count: number;
						claim_generation: number;
						resume_grant_state: string;
						attempt_manifest_fingerprint: string | null;
						attempt_provider_fingerprint: string | null;
						attempt_max_memory_items: number | null;
						resume_target_role: string | null;
						resume_target_provider_fingerprint: string | null;
						resume_target_manifest_fingerprint: string | null;
						resume_signal_kind: string | null;
						resume_signal_disposition: string | null;
				  }
				| undefined;
			if (row?.status !== "queued") return null;
			if (
				!hasExactRawEventSourceRange(this.db, {
					source: row.source,
					streamId: row.stream_id,
					startEventSeq: Number(row.start_event_seq),
					endEventSeq: Number(row.end_event_seq),
				})
			) {
				return null;
			}
			const usedResumeGrant = row.resume_grant_state === "pending";
			if (
				usedResumeGrant &&
				(row.resume_signal_disposition !== "accepted" ||
					row.resume_target_role !== "summary" ||
					row.resume_target_provider_fingerprint !== providerFingerprint ||
					row.resume_target_manifest_fingerprint !== manifestFingerprint)
			) {
				throw new Error("resume grant target is invalid");
			}
			if (
				maxMemoryItemsPerDerivation === 17 &&
				(!usedResumeGrant ||
					row.resume_signal_kind !== "validated_configuration_activation" ||
					Number(row.attempt_count) < 1 ||
					row.attempt_max_memory_items !== 16 ||
					manifest?.baseConfigurationFingerprint !== row.attempt_manifest_fingerprint ||
					providerFingerprint !== row.attempt_provider_fingerprint)
			) {
				throw new Error("maxMemoryItemsPerDerivation requires the bound recovery successor");
			}
			if (!usedResumeGrant && Number(row.attempt_count) >= Number(row.retry_limit)) return null;
			const attemptCount = Number(row.attempt_count) + 1;
			const claimGeneration = Number(row.claim_generation) + 1;
			const attemptFingerprint = `sha256:${sha256(
				`free-mem:processing-attempt:v1\0${canonicalMutationJson({
					jobId: Number(row.id),
					source: row.source,
					streamId: row.stream_id,
					startEventSeq: Number(row.start_event_seq),
					endEventSeq: Number(row.end_event_seq),
					attemptCount,
					claimGeneration,
					manifestFingerprint,
					providerFingerprint,
				})}`,
			)}`;
			const now = nowIso();
			const result = this.db
				.prepare(
					`UPDATE raw_event_flush_batches
					 SET status = 'processing', attempt_count = ?, claim_generation = ?,
						 attempt_manifest_fingerprint = ?, attempt_provider_fingerprint = ?,
						 attempt_fingerprint = ?, attempt_max_memory_items = ?,
						 resume_grant_state = CASE WHEN resume_grant_state = 'pending' THEN 'consumed' ELSE resume_grant_state END,
						 resume_grant_consumed_at = CASE WHEN resume_grant_state = 'pending' THEN ? ELSE resume_grant_consumed_at END,
						 safe_error_code = NULL, updated_at = ?
					 WHERE id = ? AND status = 'queued' AND claim_generation = ?`,
				)
				.run(
					attemptCount,
					claimGeneration,
					manifestFingerprint,
					providerFingerprint,
					attemptFingerprint,
					maxMemoryItemsPerDerivation,
					now,
					now,
					input.jobId,
					row.claim_generation,
				);
			if (result.changes !== 1) return null;
			const claim: RawEventJobClaim = {
				jobId: Number(row.id),
				source: row.source,
				streamId: row.stream_id,
				startEventSeq: Number(row.start_event_seq),
				endEventSeq: Number(row.end_event_seq),
				attemptCount,
				claimGeneration,
				attemptFingerprint,
				manifestFingerprint,
				providerFingerprint,
				maxMemoryItemsPerDerivation,
				usedResumeGrant,
			};
			const allSources = this.loadBoundRawEventSources({
				source: row.source,
				streamId: row.stream_id,
				startEventSeq: Number(row.start_event_seq),
				endEventSeq: Number(row.end_event_seq),
			});
			const projectedSources = this.projectBoundRawEventSources(boundary, allSources);
			const projectedRepositories = new Set(
				projectedSources.map((source) => source.repositoryIdentity),
			);
			const set: ProjectedSourceSetV1 = Object.freeze({
				version: 1,
				jobId: claim.jobId,
				claimGeneration: claim.claimGeneration,
				attemptFingerprint: claim.attemptFingerprint,
				destinationBoundaryFingerprint: boundaryFingerprint,
				repositoryIdentity:
					projectedRepositories.size === 1 ? ([...projectedRepositories][0] ?? null) : null,
				sources: Object.freeze(
					projectedSources.map((source, ordinal) =>
						Object.freeze({
							ordinal,
							eventId: source.eventId,
							sensitivity: source.sensitivity,
							repositoryIdentity: source.repositoryIdentity,
							redactedPayload: canonicalMutationJson(
								providerRedactedPayload(fromJson(source.payloadJson)),
							),
							payloadDigest: source.payloadDigest,
							payloadDigestVersion: source.payloadDigestVersion,
						}),
					),
				),
			});
			this.rawEventClaimProjections.set(claim, {
				boundary,
				set,
				allSources: Object.freeze(allSources),
				projectedSources: Object.freeze(projectedSources),
			});
			return claim;
		})();
	}

	requeueFailedRawEventFlushJob(jobId: number): boolean {
		const result = this.db
			.prepare(
				`UPDATE raw_event_flush_batches SET status = 'queued', updated_at = ?
				 WHERE id = ? AND status = 'failed' AND attempt_count < retry_limit`,
			)
			.run(nowIso(), jobId);
		return result.changes === 1;
	}

	failRawEventFlushJob(input: {
		jobId: number;
		claimGeneration: number;
		attemptFingerprint: string;
		safeErrorCode: string;
		diagnostic?: Record<string, unknown> | null;
	}): { status: "failed" | "retry_exhausted" } {
		if (!/^[a-z][a-z0-9_]{0,63}$/.test(input.safeErrorCode)) {
			throw new Error("safeErrorCode is invalid");
		}
		return this.db.transaction((): { status: "failed" | "retry_exhausted" } => {
			const row = this.db
				.prepare(
					`SELECT attempt_count, retry_limit, resume_grant_state
					 FROM raw_event_flush_batches
					 WHERE id = ? AND status = 'processing' AND claim_generation = ? AND attempt_fingerprint = ?`,
				)
				.get(input.jobId, input.claimGeneration, input.attemptFingerprint) as
				| { attempt_count: number; retry_limit: number; resume_grant_state: string }
				| undefined;
			if (!row) throw new StaleRawEventClaimError();
			const status =
				Number(row.attempt_count) >= Number(row.retry_limit) ||
				row.resume_grant_state === "consumed"
					? "retry_exhausted"
					: "failed";
			const result = this.db
				.prepare(
					`UPDATE raw_event_flush_batches
					 SET status = ?, safe_error_code = ?, egress_diagnostic_json = ?, updated_at = ?
					 WHERE id = ? AND status = 'processing' AND claim_generation = ? AND attempt_fingerprint = ?`,
				)
				.run(
					status,
					input.safeErrorCode,
					input.diagnostic ? toJson(input.diagnostic) : null,
					nowIso(),
					input.jobId,
					input.claimGeneration,
					input.attemptFingerprint,
				);
			if (result.changes !== 1) throw new StaleRawEventClaimError();
			return { status };
		})();
	}

	recoverStuckRawEventFlushJobs(olderThanIso: string, limit = 100): number {
		const result = this.db
			.prepare(
				`WITH candidates AS (
					SELECT id FROM raw_event_flush_batches
					WHERE status = 'processing' AND updated_at < ? ORDER BY updated_at LIMIT ?
				 )
				 UPDATE raw_event_flush_batches
				 SET status = CASE
						WHEN attempt_count >= retry_limit OR resume_grant_state = 'consumed'
						THEN 'retry_exhausted' ELSE 'failed' END,
					safe_error_code = 'stale_claim', updated_at = ?
				 WHERE id IN (SELECT id FROM candidates)`,
			)
			.run(olderThanIso, limit, nowIso());
		return result.changes;
	}

	loadRawEventFlushJobEvents(claim: RawEventJobClaim): Record<string, unknown>[] {
		const projection = this.rawEventClaimProjections.get(claim);
		if (!projection) throw new StaleRawEventClaimError();
		return projection.projectedSources.map((source) => ({
			...fromJson(source.payloadJson),
			event_id: source.eventId,
			event_seq: source.eventSeq,
			event_type: source.eventType,
			timestamp_wall_ms: source.tsWallMs,
			timestamp_mono_ms: source.tsMonoMs,
			sensitivity: source.sensitivity,
			repository_identity: source.repositoryIdentity,
			capture_state: source.captureState,
			safe_error_code: source.safeErrorCode,
		}));
	}

	rawEventFlushClaimSourceEventIds(claim: RawEventJobClaim): string[] {
		const projection = this.rawEventClaimProjections.get(claim);
		if (!projection) throw new StaleRawEventClaimError();
		return projection.allSources.map((source) => source.eventId);
	}

	rawEventFlushClaimProjectedSourceSet(claim: RawEventJobClaim): ProjectedSourceSetV1 {
		const projection = this.rawEventClaimProjections.get(claim);
		if (!projection) throw new StaleRawEventClaimError();
		return projection.set;
	}

	private validateRawEventFlushCompletion(input: {
		claim: RawEventJobClaim;
		sourceEventIds: string[];
	}): {
		source: string;
		streamId: string;
		start: number;
		end: number;
		frontierAdvanced: boolean;
		attemptManifestFingerprint: string;
		attemptProviderFingerprint: string;
		attemptMaxMemoryItems: 16 | 17;
		sources: Array<{
			eventId: string;
			sensitivity: SensitivityV1;
			repositoryIdentity: string | null;
			captureState: RawEventCaptureState;
		}>;
	} {
		const bound = this.rawEventClaimProjections.get(input.claim);
		if (
			!bound ||
			destinationBoundaryFingerprint(bound.boundary) !== bound.set.destinationBoundaryFingerprint
		) {
			throw new StaleRawEventClaimError();
		}
		const row = this.db
			.prepare(
				`SELECT source, stream_id, start_event_seq, end_event_seq, frontier_already_advanced,
					attempt_manifest_fingerprint, attempt_provider_fingerprint,
					attempt_max_memory_items
				 FROM raw_event_flush_batches
				 WHERE id = ? AND status = 'processing' AND claim_generation = ? AND attempt_fingerprint = ?`,
			)
			.get(input.claim.jobId, input.claim.claimGeneration, input.claim.attemptFingerprint) as
			| {
					source: string;
					stream_id: string;
					start_event_seq: number;
					end_event_seq: number;
					frontier_already_advanced: number;
					attempt_manifest_fingerprint: string | null;
					attempt_provider_fingerprint: string | null;
					attempt_max_memory_items: number | null;
			  }
			| undefined;
		if (!row) throw new StaleRawEventClaimError();
		const currentSources = this.loadBoundRawEventSources({
			source: row.source,
			streamId: row.stream_id,
			startEventSeq: Number(row.start_event_seq),
			endEventSeq: Number(row.end_event_seq),
		});
		const sameSource = (left: BoundRawEventSource, right: BoundRawEventSource) =>
			left.eventId === right.eventId &&
			left.eventSeq === right.eventSeq &&
			left.eventType === right.eventType &&
			left.payloadJson === right.payloadJson &&
			left.sensitivity === right.sensitivity &&
			left.repositoryIdentity === right.repositoryIdentity &&
			left.captureState === right.captureState &&
			left.safeErrorCode === right.safeErrorCode &&
			left.payloadDigestVersion === right.payloadDigestVersion &&
			left.payloadDigest === right.payloadDigest;
		const currentProjected = this.projectBoundRawEventSources(bound.boundary, currentSources);
		const expected = currentSources.map((source) => source.eventId);
		if (
			expected.length !== Number(row.end_event_seq) - Number(row.start_event_seq) + 1 ||
			expected.length !== input.sourceEventIds.length ||
			expected.some((eventId, index) => eventId !== input.sourceEventIds[index]) ||
			currentSources.length !== bound.allSources.length ||
			currentSources.some(
				(source, index) => !sameSource(source, bound.allSources[index] as BoundRawEventSource),
			) ||
			currentProjected.length !== bound.projectedSources.length ||
			currentProjected.some(
				(source, index) =>
					!sameSource(source, bound.projectedSources[index] as BoundRawEventSource),
			)
		) {
			throw new Error("source set mismatch");
		}
		const attemptManifestFingerprint = requiredFingerprint(
			row.attempt_manifest_fingerprint,
			"attempt_manifest_fingerprint",
		);
		const attemptProviderFingerprint = requiredFingerprint(
			row.attempt_provider_fingerprint,
			"attempt_provider_fingerprint",
		);
		if (row.attempt_max_memory_items !== 16 && row.attempt_max_memory_items !== 17) {
			throw new Error("attempt output limit is invalid");
		}
		return {
			source: row.source,
			streamId: row.stream_id,
			start: Number(row.start_event_seq),
			end: Number(row.end_event_seq),
			frontierAdvanced: row.frontier_already_advanced === 1,
			attemptManifestFingerprint,
			attemptProviderFingerprint,
			attemptMaxMemoryItems: row.attempt_max_memory_items,
			sources: currentSources.map((source) => ({
				eventId: source.eventId,
				sensitivity: source.sensitivity,
				repositoryIdentity: source.repositoryIdentity,
				captureState: source.captureState,
			})),
		};
	}

	private completeRawEventFlushJob(input: {
		claim: RawEventJobClaim;
		sourceEventIds: string[];
		completionDisposition: "memory_committed" | "privacy_skip";
		outputCount: number;
		observedOutputCount: number;
		diagnostic: Record<string, unknown>;
	}): { frontierChanged: boolean } {
		const validated = this.validateRawEventFlushCompletion(input);
		let frontierChanged = false;
		if (!validated.frontierAdvanced) {
			const frontier = this.db
				.prepare(
					`UPDATE raw_event_sessions SET last_flushed_event_seq = ?, updated_at = ?
					 WHERE source = ? AND stream_id = ? AND last_flushed_event_seq = ?`,
				)
				.run(validated.end, nowIso(), validated.source, validated.streamId, validated.start - 1);
			if (frontier.changes !== 1) throw new Error("frontier mismatch");
			frontierChanged = true;
		}
		const completed = this.db
			.prepare(
				`UPDATE raw_event_flush_batches
				 SET status = 'completed', completion_disposition = ?, output_count = ?,
					observed_output_count = ?, egress_diagnostic_json = ?, safe_error_code = NULL,
					updated_at = ?
				 WHERE id = ? AND status = 'processing' AND claim_generation = ? AND attempt_fingerprint = ?`,
			)
			.run(
				input.completionDisposition,
				input.outputCount,
				input.observedOutputCount,
				toJson(input.diagnostic),
				nowIso(),
				input.claim.jobId,
				input.claim.claimGeneration,
				input.claim.attemptFingerprint,
			);
		if (completed.changes !== 1) throw new StaleRawEventClaimError();
		return { frontierChanged };
	}

	completeRawEventFlushJobPrivacySkip(input: {
		claim: RawEventJobClaim;
		sourceEventIds: string[];
		projection: RawEventPrivacyProjection;
		diagnostic: Record<string, unknown>;
	}): { frontierChanged: boolean } {
		return this.db.transaction(() => {
			this.validateRawEventFlushCompletion(input);
			const bound = this.rawEventClaimProjections.get(input.claim);
			if (
				bound?.projectedSources.length !== 0 ||
				input.projection.eligibleSourceEventIds.length !== 0 ||
				input.projection.omittedSourceEventIds.length !== input.sourceEventIds.length ||
				input.projection.omittedSourceEventIds.some(
					(eventId, index) => eventId !== input.sourceEventIds[index],
				)
			) {
				throw new Error("privacy projection is not all-ineligible");
			}
			return this.completeRawEventFlushJob({
				...input,
				completionDisposition: "privacy_skip",
				outputCount: 0,
				observedOutputCount: 0,
			});
		})();
	}

	completeRawEventFlushJobMemory(
		input: {
			claim: RawEventJobClaim;
			sourceEventIds: string[];
			observedOutputCount: number;
			diagnostic: Record<string, unknown>;
		},
		persist: (
			newMemoryIdFloor: number,
			derivation: RawEventDerivationContext,
		) => RawEventMemoryCompletion[],
	): { frontierChanged: boolean; memoryIds: number[] } {
		if (
			!Number.isInteger(input.claim.maxMemoryItemsPerDerivation) ||
			(input.claim.maxMemoryItemsPerDerivation !== 16 &&
				input.claim.maxMemoryItemsPerDerivation !== 17) ||
			!Number.isInteger(input.observedOutputCount) ||
			input.observedOutputCount < 0 ||
			input.observedOutputCount > input.claim.maxMemoryItemsPerDerivation
		) {
			throw new ProcessingOutputLimitError();
		}
		return this.db.transaction(() => {
			const validated = this.validateRawEventFlushCompletion(input);
			const boundProjection = this.rawEventClaimProjections.get(input.claim);
			if (!boundProjection) throw new StaleRawEventClaimError();
			if (
				input.claim.manifestFingerprint !== validated.attemptManifestFingerprint ||
				input.claim.providerFingerprint !== validated.attemptProviderFingerprint ||
				input.claim.maxMemoryItemsPerDerivation !== validated.attemptMaxMemoryItems
			) {
				throw new Error("attempt binding mismatch");
			}
			const resolveDerivedProvenance = (
				citations: readonly SourceCitationV1[],
			): DerivedMemoryProvenance => {
				if (
					citations.length === 0 ||
					citations.length > boundProjection.set.sources.length ||
					citations.some(
						(citation, index) =>
							!Number.isSafeInteger(citation.source) ||
							citation.source < 0 ||
							(index > 0 && citation.source <= (citations[index - 1]?.source ?? -1)) ||
							(citation.start === null) !== (citation.end === null),
					)
				) {
					throw new Error("memory citation set is invalid");
				}
				const cited = [];
				const sourceSpans: SourceSpanV1[] = [];
				for (const citation of citations) {
					const source = boundProjection.set.sources[citation.source];
					if (!source || source.ordinal !== citation.source) {
						throw new Error("memory citation set is invalid");
					}
					cited.push(source);
					const byteLength = Buffer.byteLength(source.redactedPayload, "utf8");
					const startByte = citation.start ?? 0;
					const endByte = citation.end ?? byteLength;
					if (
						!Number.isSafeInteger(startByte) ||
						!Number.isSafeInteger(endByte) ||
						startByte < 0 ||
						startByte >= endByte ||
						endByte > byteLength ||
						!isUtf8Boundary(source.redactedPayload, startByte) ||
						!isUtf8Boundary(source.redactedPayload, endByte)
					) {
						throw new Error("memory citation span is invalid");
					}
					sourceSpans.push({ eventId: source.eventId, startByte, endByte });
				}
				const repositoryIdentity = cited[0]?.repositoryIdentity ?? null;
				if (
					repositoryIdentity === null ||
					cited.some((source) => source.repositoryIdentity !== repositoryIdentity)
				) {
					throw new Error("memory citations require one repository identity");
				}
				// Code-unit comparison, not localeCompare: this order feeds the
				// lineage/derivation hashes, which must be identical across devices
				// regardless of locale/ICU configuration.
				const canonicalSpans = [...sourceSpans].sort(
					(a, b) =>
						(a.eventId < b.eventId ? -1 : a.eventId > b.eventId ? 1 : 0) ||
						a.startByte - b.startByte ||
						a.endByte - b.endByte,
				);
				const sourceEventIds = cited.map((source) => source.eventId);
				const lineageId = sha256(
					`free-mem:memory-lineage:v1\0${canonicalMutationJson({
						repositoryScope: repositoryIdentity,
						sourceSpans: canonicalSpans,
					})}`,
				);
				const derivationKey = sha256(
					`free-mem:memory-derivation:v1\0${canonicalMutationJson({
						lineageId,
						manifestFingerprint: validated.attemptManifestFingerprint,
						providerFingerprint: validated.attemptProviderFingerprint,
					})}`,
				);
				return {
					source: validated.source,
					streamId: validated.streamId,
					repositoryIdentity,
					// Floor over the FULL projected set, not just the cited ordinals:
					// the provider's prompt carried every projected source (and the
					// same content again in transcript/session-context blocks), so an
					// output that omits a restricted ordinal from its citations must
					// not launder restricted-derived content into 'eligible'.
					sensitivity: boundProjection.set.sources.reduce<SensitivityV1>(
						(strongest, source) => strongestSensitivity(strongest, source.sensitivity),
						"eligible",
					),
					dedupSourceIdentity: sha256(
						`free-mem:derived-memory-source:v1\0${canonicalMutationJson({
							source: validated.source,
							streamId: validated.streamId,
							derivationKey,
						})}`,
					),
					sourceEventIds,
					sourceSpans: canonicalSpans,
					lineageId,
					derivationKey,
				};
			};
			const maxMemoryIdBefore = Number(
				(
					this.db.prepare("SELECT COALESCE(MAX(id), 0) AS id FROM memory_items").get() as {
						id: number;
					}
				).id,
			);
			const issuedResults = new Set<RawEventMemoryCompletion>();
			const responseAnchors = new Set<string>();
			const completions = persist(maxMemoryIdBefore, {
				remember: ({ sourceCitations, ...memory }) => {
					const provenance = resolveDerivedProvenance(sourceCitations);
					const anchorKey = toJson(provenance.sourceSpans);
					if (responseAnchors.has(anchorKey)) {
						throw new Error("provider response contains a duplicate source anchor");
					}
					responseAnchors.add(anchorKey);
					const anchors = this.db
						.prepare(
							`SELECT memory_items.id, memory_items.active, memory_items.deleted_at,
								memory_items.source_spans_json, memory_items.derivation_key,
								memory_items.revision_ordinal
							 FROM memory_items
							 WHERE memory_items.repository_identity = ?
								AND memory_items.source_spans_json IS NOT NULL
								AND json_extract(memory_items.metadata_json, '$.derived_source') = ?
								AND json_extract(memory_items.metadata_json, '$.derived_stream_id') = ?`,
						)
						.all(provenance.repositoryIdentity, validated.source, validated.streamId) as Array<{
						id: number;
						active: number;
						deleted_at: string | null;
						source_spans_json: string;
						derivation_key: string | null;
						revision_ordinal: number | null;
					}>;
					const parsedAnchors = anchors.flatMap((anchor) => {
						try {
							const spans = JSON.parse(anchor.source_spans_json) as SourceSpanV1[];
							return Array.isArray(spans) ? [{ ...anchor, spans }] : [];
						} catch {
							return [];
						}
					});
					// Prefer the ACTIVE row among span-equal anchors: after a supersede
					// the stale inactive twin shares the same spans, and picking it
					// would skip the dedupe branch and then trip the dedup_key
					// collision throw on every retry.
					const exactCandidates = parsedAnchors.filter((anchor) =>
						sourceSpansEqual(anchor.spans, provenance.sourceSpans),
					);
					const exact = exactCandidates.find((anchor) => anchor.active === 1) ?? exactCandidates[0];
					if (
						parsedAnchors.some(
							(anchor) =>
								anchor.deleted_at !== null &&
								sourceSpansOverlap(anchor.spans, provenance.sourceSpans),
						)
					) {
						const result = Object.freeze({
							memoryId: null,
							disposition: "suppressed" as const,
						});
						issuedResults.add(result);
						return result;
					}
					if (
						parsedAnchors.some(
							(anchor) =>
								anchor.active === 1 &&
								!sourceSpansEqual(anchor.spans, provenance.sourceSpans) &&
								sourceSpansOverlap(anchor.spans, provenance.sourceSpans),
						)
					) {
						throw new Error("provider response source anchor overlaps an active memory");
					}
					if (exact?.active === 1 && exact.derivation_key === provenance.derivationKey) {
						this.strengthenDerivedDuplicate(exact.id, provenance);
						const result = Object.freeze({
							memoryId: exact.id,
							disposition: "deduplicated" as const,
						});
						issuedResults.add(result);
						return result;
					}
					const memoryId = this.rememberInternal(
						memory.sessionId,
						memory.kind,
						memory.title,
						memory.bodyText,
						memory.confidence ?? 0.5,
						memory.tags,
						memory.metadata,
						provenance,
					);
					if (memoryId <= maxMemoryIdBefore) {
						throw new Error("derived memory insertion unexpectedly deduplicated");
					}
					const revisionOrdinal =
						Math.max(0, ...parsedAnchors.map((anchor) => Number(anchor.revision_ordinal ?? 0))) + 1;
					const stored = this.db
						.prepare("SELECT title, body_text FROM memory_items WHERE id = ?")
						.get(memoryId) as { title: string; body_text: string } | undefined;
					if (!stored) throw new Error("derived memory insert is missing");
					const revisionId = `revision:${sha256(
						`free-mem:memory-revision:v1\0${canonicalMutationJson({
							lineageId: provenance.lineageId,
							derivationKey: provenance.derivationKey,
							title: stored.title,
							bodyText: stored.body_text,
						})}`,
					)}`;
					const supersededId = exact?.active === 1 ? exact.id : null;
					this.db
						.prepare(
							`UPDATE memory_items
							 SET lineage_id = ?, revision_id = ?, revision_ordinal = ?,
								supersedes_memory_id = ?, derivation_key = ?, source_event_ids_json = ?,
								source_spans_json = ?, manifest_fingerprint = ?, provider_fingerprint = ?,
								attempt_fingerprint = ?
							 WHERE id = ?`,
						)
						.run(
							provenance.lineageId,
							revisionId,
							revisionOrdinal,
							supersededId,
							provenance.derivationKey,
							toJson(provenance.sourceEventIds),
							toJson(provenance.sourceSpans),
							validated.attemptManifestFingerprint,
							validated.attemptProviderFingerprint,
							input.claim.attemptFingerprint,
							memoryId,
						);
					if (supersededId !== null) {
						this.db
							.prepare("UPDATE memory_items SET active = 0 WHERE id = ? AND active = 1")
							.run(supersededId);
					}
					const result = Object.freeze({
						memoryId,
						disposition: "inserted" as const,
					});
					issuedResults.add(result);
					return result;
				},
			});
			if (
				completions.length !== issuedResults.size ||
				completions.length > input.observedOutputCount ||
				completions.some((completion) => !issuedResults.has(completion))
			) {
				throw new Error("output count mismatch");
			}
			const memoryIds = new Set<number>();
			let outputCount = 0;
			for (const completion of completions) {
				if (completion.memoryId !== null) {
					if (memoryIds.has(completion.memoryId)) throw new Error("memory completion is invalid");
					memoryIds.add(completion.memoryId);
				}
				if (completion.disposition === "inserted") outputCount++;
			}
			const completion = this.completeRawEventFlushJob({
				...input,
				completionDisposition: "memory_committed",
				outputCount,
			});
			return { ...completion, memoryIds: [...memoryIds] };
		})();
	}

	getProcessingJobForDoctor(jobId: number): DoctorProcessingJobProjection | null {
		const row = this.db
			.prepare(
				`SELECT id, status, admission_manifest_fingerprint, admission_provider_fingerprint,
					retry_limit, attempt_count, claim_generation, attempt_manifest_fingerprint,
					attempt_provider_fingerprint, attempt_fingerprint, resume_grant_state,
					last_resume_sequence
				 FROM raw_event_flush_batches WHERE id = ?`,
			)
			.get(jobId) as
			| {
					id: number;
					status: DoctorProcessingJobProjection["state"];
					admission_manifest_fingerprint: string | null;
					admission_provider_fingerprint: string | null;
					retry_limit: number;
					attempt_count: number;
					claim_generation: number;
					attempt_manifest_fingerprint: string | null;
					attempt_provider_fingerprint: string | null;
					attempt_fingerprint: string | null;
					resume_grant_state: RawEventFlushBatch["resume_grant_state"];
					last_resume_sequence: number;
			  }
			| undefined;
		if (!row) return null;
		return {
			jobId: Number(row.id),
			component: "summary",
			state: row.status,
			admission: {
				manifestFingerprint: row.admission_manifest_fingerprint,
				providerFingerprint: row.admission_provider_fingerprint,
				retryLimit: Number(row.retry_limit),
			},
			attempt: {
				count: Number(row.attempt_count),
				claimGeneration: Number(row.claim_generation),
				manifestFingerprint: row.attempt_manifest_fingerprint,
				providerFingerprint: row.attempt_provider_fingerprint,
				fingerprint: row.attempt_fingerprint,
			},
			resume: {
				grantState: row.resume_grant_state,
				lastSequence: Number(row.last_resume_sequence),
			},
			retryTarget: null,
			nextAction: row.status === "retry_exhausted" ? "activate_valid_manifest" : "none",
		};
	}

	private matchingResumeProducerReceipt(
		input: ResumeProducerReceiptIdentity,
	): ResumeProducerReceiptRow | undefined {
		if (!Number.isSafeInteger(input.producerSequence) || input.producerSequence <= 0) {
			throw new ProcessingResumeError("invalid_signal", "Resume producer receipt is invalid.");
		}
		const prior = this.db
			.prepare(
				`SELECT producer_kind, configuration_fingerprint, provider_fingerprint,
					producer_sequence, fanout_count, target_job_ids_json
				 FROM processing_resume_producer_receipts WHERE receipt_id = ?`,
			)
			.get(input.receiptId) as ResumeProducerReceiptRow | undefined;
		if (
			prior &&
			(prior.producer_kind !== input.producerKind ||
				prior.configuration_fingerprint !== input.manifestFingerprint ||
				prior.provider_fingerprint !== input.providerFingerprint ||
				Number(prior.producer_sequence) !== input.producerSequence)
		) {
			throw new ProcessingResumeError(
				"invalid_signal",
				"Resume producer receipt identity does not match.",
			);
		}
		if (prior) {
			const targetJobIds = parseResumeProducerTargetJobIds(prior.target_job_ids_json);
			if (input.targetJobId !== undefined && !targetJobIds.includes(input.targetJobId)) {
				throw new ProcessingResumeError(
					"invalid_signal",
					"Resume producer receipt is not bound to this job.",
				);
			}
		}
		if (prior?.producer_kind === "user_confirmed_doctor_retry" && input.targetJobId !== undefined) {
			const priorSignal = this.db
				.prepare(
					"SELECT job_id FROM processing_resume_signals WHERE producer_receipt_id = ? LIMIT 1",
				)
				.get(input.receiptId) as { job_id: number } | undefined;
			if (priorSignal && Number(priorSignal.job_id) !== input.targetJobId) {
				throw new ProcessingResumeError(
					"invalid_signal",
					"Doctor retry receipt is already bound to another job.",
				);
			}
		}
		return prior;
	}

	private applyResumeSignalInTransaction(
		input: ResumeSignalV1,
		producerSequence?: number,
	): ResumeSignalResult {
		if (
			!/^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$/.test(input.signalId) ||
			!/^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$/.test(input.producerReceiptId) ||
			!Number.isInteger(input.targetJobId) ||
			input.targetJobId <= 0 ||
			!Number.isSafeInteger(input.sequence) ||
			input.sequence <= 0
		) {
			throw new ProcessingResumeError("invalid_signal", "Resume signal is invalid.");
		}
		const manifestFingerprint = requiredFingerprint(
			input.manifestFingerprint,
			"target_manifest_fingerprint",
		);
		const providerFingerprint = requiredFingerprint(
			input.providerFingerprint,
			"target_provider_fingerprint",
		);
		const duplicate = this.db
			.prepare(
				`SELECT signal.signal_id, signal.producer_receipt_id, signal.sequence,
					signal.target_role, signal.target_provider_fingerprint,
					signal.target_manifest_fingerprint, signal.kind,
					receipt.producer_kind, receipt.configuration_fingerprint,
					receipt.provider_fingerprint, receipt.producer_sequence
				 FROM processing_resume_signals AS signal
				 JOIN processing_resume_producer_receipts AS receipt
				   ON receipt.receipt_id = signal.producer_receipt_id
				 WHERE signal.job_id = ?
				   AND (signal.signal_id = ? OR signal.producer_receipt_id = ?) LIMIT 1`,
			)
			.get(input.targetJobId, input.signalId, input.producerReceiptId) as
			| {
					signal_id: string;
					producer_receipt_id: string;
					sequence: number;
					target_role: string;
					target_provider_fingerprint: string;
					target_manifest_fingerprint: string;
					kind: ResumeSignalKind;
					producer_kind: ResumeSignalKind;
					configuration_fingerprint: string;
					provider_fingerprint: string;
					producer_sequence: number;
			  }
			| undefined;
		if (duplicate) {
			if (
				duplicate.producer_receipt_id !== input.producerReceiptId ||
				Number(duplicate.sequence) !== input.sequence ||
				duplicate.target_role !== input.targetRole ||
				duplicate.target_provider_fingerprint !== providerFingerprint ||
				duplicate.target_manifest_fingerprint !== manifestFingerprint ||
				duplicate.kind !== input.kind ||
				duplicate.producer_kind !== input.kind ||
				duplicate.configuration_fingerprint !== manifestFingerprint ||
				duplicate.provider_fingerprint !== providerFingerprint ||
				(producerSequence !== undefined && Number(duplicate.producer_sequence) !== producerSequence)
			) {
				throw new ProcessingResumeError("invalid_signal", "Resume signal identity does not match.");
			}
			this.db
				.prepare(
					`UPDATE raw_event_flush_batches
					 SET last_resume_signal_disposition = 'duplicate', updated_at = ?
					 WHERE id = ?`,
				)
				.run(nowIso(), input.targetJobId);
			const projection = this.getProcessingJobForDoctor(input.targetJobId);
			return {
				jobId: input.targetJobId,
				signalId: duplicate.signal_id,
				producerReceiptId: duplicate.producer_receipt_id,
				sequence: Number(duplicate.sequence),
				disposition: "duplicate",
				grantState: projection?.resume.grantState ?? "none",
			};
		}
		const effectiveProducerSequence = producerSequence ?? input.sequence;
		const priorReceipt = this.matchingResumeProducerReceipt({
			receiptId: input.producerReceiptId,
			producerKind: input.kind,
			producerSequence: effectiveProducerSequence,
			manifestFingerprint,
			providerFingerprint,
			targetJobId: input.targetJobId,
		});
		const job = this.db
			.prepare(
				`SELECT source, stream_id, start_event_seq, end_event_seq, status,
					admission_manifest_fingerprint, admission_provider_fingerprint,
					attempt_manifest_fingerprint, attempt_provider_fingerprint, legacy_recovery_state,
					resume_grant_state, last_resume_sequence
				 FROM raw_event_flush_batches WHERE id = ?`,
			)
			.get(input.targetJobId) as ResumeSignalJobRow | undefined;
		const configurationChanged =
			job !== undefined &&
			(job.attempt_provider_fingerprint !== providerFingerprint ||
				job.attempt_manifest_fingerprint !== manifestFingerprint);
		const attemptMatches =
			(job?.attempt_provider_fingerprint === providerFingerprint &&
				job.attempt_manifest_fingerprint === manifestFingerprint) ||
			(input.kind === "user_confirmed_doctor_retry" && isLegacyUnknownAttempt(job));
		const otherwiseValidPending =
			job !== undefined &&
			input.targetRole === "summary" &&
			input.sequence > Number(job.last_resume_sequence) &&
			(input.kind === "validated_configuration_activation" ? configurationChanged : attemptMatches);
		if (job?.resume_grant_state === "pending" && otherwiseValidPending) {
			return {
				jobId: input.targetJobId,
				signalId: input.signalId,
				producerReceiptId: input.producerReceiptId,
				sequence: input.sequence,
				disposition: "grant_pending",
				grantState: "pending",
			};
		}
		const disposition = resolveResumeSignalDisposition(
			input,
			job,
			manifestFingerprint,
			providerFingerprint,
		);
		if (
			disposition === "accepted" &&
			job &&
			!hasExactRawEventSourceRange(this.db, {
				source: job.source,
				streamId: job.stream_id,
				startEventSeq: Number(job.start_event_seq),
				endEventSeq: Number(job.end_event_seq),
			})
		) {
			throw new ProcessingResumeError(
				"stale_snapshot",
				"Processing job source range is unavailable.",
			);
		}

		const now = nowIso();
		if (!priorReceipt) {
			this.db
				.prepare(
					`INSERT INTO processing_resume_producer_receipts(
						receipt_id, producer_kind, configuration_fingerprint, provider_fingerprint,
						producer_sequence, fanout_count, target_job_ids_json, created_at
					 ) VALUES (?, ?, ?, ?, ?, 0, ?, ?)`,
				)
				.run(
					input.producerReceiptId,
					input.kind,
					manifestFingerprint,
					providerFingerprint,
					effectiveProducerSequence,
					toJson([input.targetJobId]),
					now,
				);
		}
		const grantId =
			disposition === "accepted"
				? stableReceiptId("resume-grant-v1", "free-mem:resume-grant:v1", {
						jobId: input.targetJobId,
						producerReceiptId: input.producerReceiptId,
						signalId: input.signalId,
					})
				: null;
		this.db
			.prepare(
				`INSERT INTO processing_resume_signals(
					signal_id, job_id, producer_receipt_id, sequence, target_role,
					target_provider_fingerprint, target_manifest_fingerprint, kind,
					disposition, grant_id, created_at
				 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			)
			.run(
				input.signalId,
				input.targetJobId,
				input.producerReceiptId,
				input.sequence,
				input.targetRole,
				providerFingerprint,
				manifestFingerprint,
				input.kind,
				disposition,
				grantId,
				now,
			);
		if (job) {
			if (disposition === "accepted") {
				this.db
					.prepare(
						`UPDATE raw_event_flush_batches
						 SET status = 'queued', resume_grant_id = ?, resume_grant_reason = ?, resume_grant_state = 'pending',
							resume_grant_consumed_at = NULL, last_resume_signal_id = ?,
							last_resume_sequence = ?, last_resume_signal_disposition = 'accepted', updated_at = ?
						 WHERE id = ? AND status = 'retry_exhausted' AND resume_grant_state != 'pending'`,
					)
					.run(grantId, input.kind, input.signalId, input.sequence, now, input.targetJobId);
			} else {
				this.db
					.prepare(
						`UPDATE raw_event_flush_batches
						 SET last_resume_signal_id = ?, last_resume_signal_disposition = ?, updated_at = ?
						 WHERE id = ?`,
					)
					.run(input.signalId, disposition, now, input.targetJobId);
			}
		}
		return {
			jobId: input.targetJobId,
			signalId: input.signalId,
			producerReceiptId: input.producerReceiptId,
			sequence: input.sequence,
			disposition,
			grantState: disposition === "accepted" ? "pending" : (job?.resume_grant_state ?? "none"),
		};
	}

	applyResumeSignal(input: ResumeSignalV1): ResumeSignalResult {
		return this.db.transaction(() => this.applyResumeSignalInTransaction(input))();
	}

	private fanoutResumeProducerInTransaction(input: {
		receiptId: string;
		producerKind: "validated_configuration_activation" | "recorded_provider_healthy_transition";
		producerSequence: number;
		manifestFingerprint: string;
		providerFingerprint: string;
	}): ResumeFanoutResult {
		const prior = this.matchingResumeProducerReceipt({
			receiptId: input.receiptId,
			producerKind: input.producerKind,
			producerSequence: input.producerSequence,
			manifestFingerprint: input.manifestFingerprint,
			providerFingerprint: input.providerFingerprint,
		});
		let latestActivationSequence: number | null = null;
		if (input.producerKind === "validated_configuration_activation") {
			const latest = this.db
				.prepare(
					`SELECT MAX(producer_sequence) AS producer_sequence
					 FROM processing_resume_producer_receipts
					 WHERE producer_kind = 'validated_configuration_activation'`,
				)
				.get() as { producer_sequence: number | null };
			latestActivationSequence =
				latest.producer_sequence === null ? null : Number(latest.producer_sequence);
			if (
				!prior &&
				latestActivationSequence !== null &&
				input.producerSequence <= latestActivationSequence
			) {
				return {
					producerReceiptId: input.receiptId,
					disposition: "stale",
					fanoutCount: 0,
					results: [],
				};
			}
		}
		const rows = this.db
			.prepare(
				`SELECT batch.id, batch.source, batch.stream_id, batch.start_event_seq,
					batch.end_event_seq, batch.attempt_manifest_fingerprint,
					batch.attempt_provider_fingerprint, batch.status, batch.resume_grant_state,
					batch.last_resume_sequence,
					EXISTS(
						SELECT 1 FROM processing_resume_signals AS signal
						WHERE signal.job_id = batch.id AND signal.producer_receipt_id = ?
					) AS receipt_applied
				 FROM raw_event_flush_batches AS batch
				 WHERE batch.status IN ('queued', 'processing', 'failed', 'retry_exhausted')
				    OR batch.resume_grant_state = 'pending'
					 ORDER BY batch.id LIMIT ?`,
			)
			.all(input.receiptId, PROCESSING_JOB_CAPACITY + 1) as Array<{
			id: number;
			source: string;
			stream_id: string;
			start_event_seq: number;
			end_event_seq: number;
			attempt_manifest_fingerprint: string | null;
			attempt_provider_fingerprint: string | null;
			status: string;
			resume_grant_state: string;
			last_resume_sequence: number;
			receipt_applied: number;
		}>;
		if (rows.length > PROCESSING_JOB_CAPACITY) {
			throw new Error("processing capacity invariant is violated");
		}
		const hasExactTargetSourceRange = (row: (typeof rows)[number]) =>
			hasExactRawEventSourceRange(this.db, {
				source: row.source,
				streamId: row.stream_id,
				startEventSeq: Number(row.start_event_seq),
				endEventSeq: Number(row.end_event_seq),
			});
		const matchesProducerTarget = (row: (typeof rows)[number]) =>
			(input.producerKind === "validated_configuration_activation"
				? row.attempt_manifest_fingerprint !== input.manifestFingerprint ||
					row.attempt_provider_fingerprint !== input.providerFingerprint
				: row.attempt_manifest_fingerprint === input.manifestFingerprint &&
					row.attempt_provider_fingerprint === input.providerFingerprint) &&
			hasExactTargetSourceRange(row);
		const initialTargets = rows
			.filter((row) => row.status === "retry_exhausted" || row.resume_grant_state === "pending")
			.filter(matchesProducerTarget);
		const frozenTargetJobIds = prior
			? parseResumeProducerTargetJobIds(prior.target_job_ids_json)
			: initialTargets.map((target) => Number(target.id));
		const frozenTargetSet = new Set(frozenTargetJobIds);
		const targets = rows
			.filter((row) => frozenTargetSet.has(Number(row.id)))
			.filter((row) => row.receipt_applied === 0)
			.filter(hasExactTargetSourceRange);
		const blockedTargets = targets.filter(
			(target) => target.status !== "retry_exhausted" || target.resume_grant_state === "pending",
		);
		const readyTargets = targets.filter(
			(target) => target.status === "retry_exhausted" && target.resume_grant_state !== "pending",
		);
		if (prior && targets.length === 0) {
			return {
				producerReceiptId: input.receiptId,
				disposition: "duplicate",
				fanoutCount: Number(prior.fanout_count),
				results: [],
			};
		}
		if (
			prior &&
			latestActivationSequence !== null &&
			input.producerSequence < latestActivationSequence
		) {
			return {
				producerReceiptId: input.receiptId,
				disposition: "stale",
				fanoutCount: Number(prior.fanout_count),
				results: [],
			};
		}
		const now = nowIso();
		if (!prior) {
			this.db
				.prepare(
					`INSERT INTO processing_resume_producer_receipts(
						receipt_id, producer_kind, configuration_fingerprint, provider_fingerprint,
						producer_sequence, fanout_count, target_job_ids_json, created_at
					 ) VALUES (?, ?, ?, ?, ?, 0, ?, ?)`,
				)
				.run(
					input.receiptId,
					input.producerKind,
					input.manifestFingerprint,
					input.providerFingerprint,
					input.producerSequence,
					toJson(frozenTargetJobIds),
					now,
				);
		}
		const results = readyTargets.map((target) => {
			const signalSequence = Number(target.last_resume_sequence) + 1;
			const signalId = stableReceiptId("resume-signal-v1", "free-mem:resume-signal:v1", {
				jobId: Number(target.id),
				producerReceiptId: input.receiptId,
				manifestFingerprint: input.manifestFingerprint,
				providerFingerprint: input.providerFingerprint,
			});
			return this.applyResumeSignalInTransaction(
				{
					signalId,
					producerReceiptId: input.receiptId,
					targetJobId: Number(target.id),
					sequence: signalSequence,
					kind: input.producerKind,
					targetRole: "summary",
					providerFingerprint: input.providerFingerprint,
					manifestFingerprint: input.manifestFingerprint,
				},
				input.producerSequence,
			);
		});
		const fanoutCount =
			Number(prior?.fanout_count ?? 0) +
			results.filter((result) => result.disposition === "accepted").length;
		this.db
			.prepare(
				"UPDATE processing_resume_producer_receipts SET fanout_count = ? WHERE receipt_id = ?",
			)
			.run(fanoutCount, input.receiptId);
		return {
			producerReceiptId: input.receiptId,
			disposition: blockedTargets.length > 0 ? "grant_pending" : "accepted",
			fanoutCount,
			results,
		};
	}

	importActivationReceipt(input: {
		receiptId: string;
		activationSequence: number;
		manifestFingerprint: string;
		providerFingerprint: string;
	}): ResumeFanoutResult {
		return this.db.transaction(() =>
			this.fanoutResumeProducerInTransaction({
				receiptId: input.receiptId,
				producerKind: "validated_configuration_activation",
				producerSequence: input.activationSequence,
				manifestFingerprint: requiredFingerprint(input.manifestFingerprint, "manifestFingerprint"),
				providerFingerprint: requiredFingerprint(input.providerFingerprint, "providerFingerprint"),
			}),
		)();
	}

	recordProviderHealth(input: {
		manifestFingerprint: string;
		providerFingerprint: string;
		health: "available" | "provider_unavailable" | "provider_tls_rejected";
	}): ResumeFanoutResult | null {
		const manifestFingerprint = requiredFingerprint(
			input.manifestFingerprint,
			"manifestFingerprint",
		);
		const providerFingerprint = requiredFingerprint(
			input.providerFingerprint,
			"providerFingerprint",
		);
		return this.db.transaction(() => {
			const nextState = input.health === "available" ? "healthy" : "unhealthy";
			const previous = this.db
				.prepare(
					`SELECT health_state, last_transition_sequence
					 FROM provider_health_states
					 WHERE configuration_fingerprint = ? AND provider_fingerprint = ?`,
				)
				.get(manifestFingerprint, providerFingerprint) as
				| { health_state: string; last_transition_sequence: number }
				| undefined;
			const now = nowIso();
			if (!previous) {
				this.db
					.prepare(
						`INSERT INTO provider_health_states(
							configuration_fingerprint, provider_fingerprint, health_state,
							last_transition_sequence, safe_error_code, updated_at
						 ) VALUES (?, ?, ?, 0, ?, ?)`,
					)
					.run(
						manifestFingerprint,
						providerFingerprint,
						nextState,
						input.health === "available" ? null : input.health,
						now,
					);
				return null;
			}
			if (previous.health_state === nextState) {
				return null;
			}
			const transitionSequence = Number(previous.last_transition_sequence) + 1;
			if (nextState === "unhealthy") {
				this.db
					.prepare(
						`UPDATE provider_health_states
						 SET health_state = 'unhealthy', last_transition_sequence = ?,
							safe_error_code = ?, updated_at = ?
						 WHERE configuration_fingerprint = ? AND provider_fingerprint = ?`,
					)
					.run(transitionSequence, input.health, now, manifestFingerprint, providerFingerprint);
				return null;
			}
			const receiptId = stableReceiptId(
				"provider-health-receipt-v1",
				"free-mem:provider-health-transition:v1",
				{ manifestFingerprint, providerFingerprint, transitionSequence },
			);
			const fanout = this.fanoutResumeProducerInTransaction({
				receiptId,
				producerKind: "recorded_provider_healthy_transition",
				producerSequence: transitionSequence,
				manifestFingerprint,
				providerFingerprint,
			});
			if (fanout.disposition === "grant_pending") return fanout;
			this.db
				.prepare(
					`UPDATE provider_health_states
					 SET health_state = 'healthy', last_transition_sequence = ?,
						last_transition_receipt_id = ?, safe_error_code = NULL, updated_at = ?
					 WHERE configuration_fingerprint = ? AND provider_fingerprint = ?`,
				)
				.run(transitionSequence, receiptId, now, manifestFingerprint, providerFingerprint);
			return fanout;
		})();
	}

	confirmDoctorRetry(input: {
		jobId: number;
		producerReceiptId: string;
		expectedRole: "summary";
		expectedProviderFingerprint: string | null;
		expectedManifestFingerprint: string | null;
		expectedAttemptCount: number;
		expectedClaimGeneration: number;
		targetProviderFingerprint: string | null;
		targetManifestFingerprint: string | null;
	}): ResumeSignalResult {
		return this.db.transaction(() => {
			const duplicate = this.db
				.prepare(
					`SELECT signal.signal_id, signal.producer_receipt_id, signal.sequence,
							signal.target_manifest_fingerprint, signal.target_provider_fingerprint
					 FROM processing_resume_signals AS signal
					 WHERE signal.job_id = ? AND signal.producer_receipt_id = ? LIMIT 1`,
				)
				.get(input.jobId, input.producerReceiptId) as
				| {
						signal_id: string;
						producer_receipt_id: string;
						sequence: number;
						target_manifest_fingerprint: string;
						target_provider_fingerprint: string;
				  }
				| undefined;
			if (duplicate) {
				return this.applyResumeSignalInTransaction({
					signalId: duplicate.signal_id,
					producerReceiptId: duplicate.producer_receipt_id,
					targetJobId: input.jobId,
					sequence: Number(duplicate.sequence),
					kind: "user_confirmed_doctor_retry",
					targetRole: "summary",
					providerFingerprint: duplicate.target_provider_fingerprint,
					manifestFingerprint: duplicate.target_manifest_fingerprint,
				});
			}
			let targetProviderFingerprint: string;
			let targetManifestFingerprint: string;
			try {
				targetProviderFingerprint = requiredFingerprint(
					input.targetProviderFingerprint,
					"target_provider_fingerprint",
				);
				targetManifestFingerprint = requiredFingerprint(
					input.targetManifestFingerprint,
					"target_manifest_fingerprint",
				);
			} catch {
				throw new ProcessingResumeError("stale_snapshot", "Displayed job state is stale.");
			}
			const job = this.getProcessingJobForDoctor(input.jobId);
			if (!job) throw new ProcessingResumeError("not_found", "Processing job was not found.");
			if (
				job.component !== input.expectedRole ||
				job.attempt.providerFingerprint !== input.expectedProviderFingerprint ||
				job.attempt.manifestFingerprint !== input.expectedManifestFingerprint ||
				job.attempt.count !== input.expectedAttemptCount ||
				job.attempt.claimGeneration !== input.expectedClaimGeneration
			) {
				throw new ProcessingResumeError("stale_snapshot", "Displayed job state is stale.");
			}
			if (job.resume.grantState === "pending") {
				const sequence = (job.resume.lastSequence ?? 0) + 1;
				this.matchingResumeProducerReceipt({
					receiptId: input.producerReceiptId,
					producerKind: "user_confirmed_doctor_retry",
					producerSequence: sequence,
					manifestFingerprint: targetManifestFingerprint,
					providerFingerprint: targetProviderFingerprint,
					targetJobId: input.jobId,
				});
				throw new ProcessingResumeError(
					"grant_pending",
					"A resume grant is already pending.",
					true,
				);
			}
			if (job.state !== "retry_exhausted") {
				throw new ProcessingResumeError("stale_snapshot", "Displayed job state is stale.");
			}
			const sequence = (job.resume.lastSequence ?? 0) + 1;
			const signalId = stableReceiptId("resume-signal-v1", "free-mem:doctor-resume-signal:v1", {
				jobId: input.jobId,
				producerReceiptId: input.producerReceiptId,
				sequence,
			});
			const result = this.applyResumeSignalInTransaction({
				signalId,
				producerReceiptId: input.producerReceiptId,
				targetJobId: input.jobId,
				sequence,
				kind: "user_confirmed_doctor_retry",
				targetRole: "summary",
				providerFingerprint: targetProviderFingerprint,
				manifestFingerprint: targetManifestFingerprint,
			});
			if (result.disposition === "grant_pending") {
				throw new ProcessingResumeError(
					"grant_pending",
					"A resume grant is already pending.",
					true,
				);
			}
			if (result.disposition !== "accepted" && result.disposition !== "duplicate") {
				throw new ProcessingResumeError("stale_snapshot", "Displayed job state is stale.");
			}
			return result;
		})();
	}

	/**
	 * Get or create a flush batch record. Returns [batchId, status].
	 * Port of get_or_create_raw_event_flush_batch().
	 */
	getOrCreateRawEventFlushBatch(
		opencodeSessionId: string,
		source: string,
		startEventSeq: number,
		endEventSeq: number,
		extractorVersion: string,
	): { batchId: number; status: string; attemptCount: number } {
		const [s, sid] = this.normalizeStreamIdentity(source, opencodeSessionId);
		const now = new Date().toISOString();

		// Atomic UPSERT to avoid SELECT+INSERT races. We intentionally do NOT
		// heartbeat claimed/running batches: their updated_at stays unchanged so
		// stuck-batch recovery can still age them out.
		const t = schema.rawEventFlushBatches;
		const row = this.d
			.insert(t)
			.values({
				source: s,
				stream_id: sid,
				opencode_session_id: sid,
				start_event_seq: startEventSeq,
				end_event_seq: endEventSeq,
				extractor_version: extractorVersion,
				status: "queued",
				created_at: now,
				updated_at: now,
			})
			.onConflictDoUpdate({
				target: [t.source, t.stream_id, t.start_event_seq, t.end_event_seq, t.extractor_version],
				set: {
					updated_at: sql`CASE
					WHEN ${t.status} = 'processing'
						THEN ${t.updated_at}
						ELSE excluded.updated_at
					END`,
				},
			})
			.returning({ id: t.id, status: t.status, attempt_count: t.attempt_count })
			.get();

		if (!row) throw new Error("Failed to create flush batch");
		return {
			batchId: Number(row.id),
			status: String(row.status),
			attemptCount: Number(row.attempt_count ?? 0),
		};
	}

	/**
	 * Attempt to claim a flush batch for processing.
	 * Returns true if successfully claimed, false if already claimed/completed.
	 * Port of claim_raw_event_flush_batch().
	 */
	claimRawEventFlushBatch(batchId: number): boolean {
		const now = new Date().toISOString();
		const row = this.d
			.update(schema.rawEventFlushBatches)
			.set({
				status: "processing",
				updated_at: now,
				attempt_count: sql`${schema.rawEventFlushBatches.attempt_count} + 1`,
			})
			.where(
				and(
					eq(schema.rawEventFlushBatches.id, batchId),
					eq(schema.rawEventFlushBatches.status, "queued"),
				),
			)
			.returning({ id: schema.rawEventFlushBatches.id })
			.get();
		return row != null;
	}

	/**
	 * Update the status of a flush batch.
	 * Port of update_raw_event_flush_batch_status().
	 */
	updateRawEventFlushBatchStatus(batchId: number, status: RawEventJobStatus): void {
		const now = new Date().toISOString();
		if (status === "failed" || status === "retry_exhausted") {
			// Preserve existing error details for terminal attempt failures.
			this.d
				.update(schema.rawEventFlushBatches)
				.set({ status, updated_at: now })
				.where(eq(schema.rawEventFlushBatches.id, batchId))
				.run();
		} else {
			// Clear error details for non-failure statuses
			this.d
				.update(schema.rawEventFlushBatches)
				.set({
					status,
					updated_at: now,
					error_message: null,
					error_type: null,
					observer_provider: null,
					observer_model: null,
					observer_runtime: null,
					observer_auth_source: null,
					observer_auth_type: null,
					observer_error_code: null,
					observer_error_message: null,
				})
				.where(eq(schema.rawEventFlushBatches.id, batchId))
				.run();
		}
	}

	/**
	 * Preserve bounded transport diagnostics without changing the canonical job state.
	 */
	recordRawEventFlushBatchDiagnostic(
		batchId: number,
		opts: {
			message: string;
			errorType: string;
			observerProvider?: string | null;
			observerModel?: string | null;
			observerRuntime?: string | null;
			observerAuthSource?: string | null;
			observerAuthType?: string | null;
			observerErrorCode?: string | null;
			observerErrorMessage?: string | null;
		},
	): void {
		const now = new Date().toISOString();
		this.d
			.update(schema.rawEventFlushBatches)
			.set({
				updated_at: now,
				error_message: opts.message,
				error_type: opts.errorType,
				observer_provider: opts.observerProvider ?? null,
				observer_model: opts.observerModel ?? null,
				observer_runtime: opts.observerRuntime ?? null,
				observer_auth_source: opts.observerAuthSource ?? null,
				observer_auth_type: opts.observerAuthType ?? null,
				observer_error_code: opts.observerErrorCode ?? null,
				observer_error_message: opts.observerErrorMessage ?? null,
			})
			.where(eq(schema.rawEventFlushBatches.id, batchId))
			.run();
	}

	/**
	 * Update the last flushed event_seq for a session.
	 * Port of update_raw_event_flush_state().
	 */
	updateRawEventFlushState(
		opencodeSessionId: string,
		lastFlushed: number,
		source = "opencode",
	): void {
		const [s, sid] = this.normalizeStreamIdentity(source, opencodeSessionId);
		const now = new Date().toISOString();
		this.d
			.insert(schema.rawEventSessions)
			.values({
				opencode_session_id: sid,
				source: s,
				stream_id: sid,
				last_flushed_event_seq: lastFlushed,
				updated_at: now,
			})
			.onConflictDoUpdate({
				target: [schema.rawEventSessions.source, schema.rawEventSessions.stream_id],
				set: {
					opencode_session_id: sql`excluded.opencode_session_id`,
					last_flushed_event_seq: sql`excluded.last_flushed_event_seq`,
					updated_at: sql`excluded.updated_at`,
				},
			})
			.run();
	}

	// Raw event ingestion methods (ports for POST /api/raw-events)

	/**
	 * Update ingest stats counters (sample + running totals).
	 * Port of _update_raw_event_ingest_stats().
	 */
	private updateRawEventIngestStats(
		inserted: number,
		skippedInvalid: number,
		skippedDuplicate: number,
		skippedConflict: number,
	): void {
		const now = nowIso();
		const skippedEvents = skippedInvalid + skippedDuplicate + skippedConflict;
		this.d
			.insert(schema.rawEventIngestSamples)
			.values({
				created_at: now,
				inserted_events: inserted,
				skipped_invalid: skippedInvalid,
				skipped_duplicate: skippedDuplicate,
				skipped_conflict: skippedConflict,
			})
			.run();
		this.d
			.insert(schema.rawEventIngestStats)
			.values({
				id: 1,
				inserted_events: inserted,
				skipped_events: skippedEvents,
				skipped_invalid: skippedInvalid,
				skipped_duplicate: skippedDuplicate,
				skipped_conflict: skippedConflict,
				updated_at: now,
			})
			.onConflictDoUpdate({
				target: schema.rawEventIngestStats.id,
				set: {
					inserted_events: sql`${schema.rawEventIngestStats.inserted_events} + excluded.inserted_events`,
					skipped_events: sql`${schema.rawEventIngestStats.skipped_events} + excluded.skipped_events`,
					skipped_invalid: sql`${schema.rawEventIngestStats.skipped_invalid} + excluded.skipped_invalid`,
					skipped_duplicate: sql`${schema.rawEventIngestStats.skipped_duplicate} + excluded.skipped_duplicate`,
					skipped_conflict: sql`${schema.rawEventIngestStats.skipped_conflict} + excluded.skipped_conflict`,
					updated_at: sql`excluded.updated_at`,
				},
			})
			.run();
	}

	private persistRawEventQuarantine(input: {
		repositoryIdentity: string | null;
		source: string;
		streamId: string;
		eventId: string;
		eventType: string;
		payload: Record<string, unknown>;
		payloadDigest: string;
		tsWallMs: number | null;
		tsMonoMs: number | null;
		captureManifestFingerprint: string | null;
		reason: "repository_identity_unknown_collision" | "redaction_degraded";
		now: string;
	}): RawEventCaptureOutcome {
		const receiptId = stableReceiptId("quarantine-receipt-v1", "free-mem:raw-event-quarantine:v1", {
			repositoryIdentity: input.repositoryIdentity ?? UNKNOWN_REPOSITORY_INDEX_SENTINEL,
			source: input.source,
			streamId: input.streamId,
			eventId: input.eventId,
			payloadDigestVersion: EVENT_PAYLOAD_DIGEST_VERSION,
			payloadDigest: input.payloadDigest,
		});
		this.db
			.prepare(
				`INSERT INTO raw_event_quarantine(
					receipt_id, repository_identity, source, stream_id, event_id, event_type,
					ts_wall_ms, ts_mono_ms, payload_json, payload_digest_version, payload_digest,
					sensitivity, capture_state, safe_error_code, capture_manifest_fingerprint,
					first_seen_at, last_seen_at, occurrence_count
				 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'secret', 'quarantined', ?, ?, ?, ?, 1)
				 ON CONFLICT(receipt_id) DO UPDATE SET
					last_seen_at = excluded.last_seen_at,
					occurrence_count = raw_event_quarantine.occurrence_count + 1`,
			)
			.run(
				receiptId,
				input.repositoryIdentity,
				input.source,
				input.streamId,
				input.eventId,
				input.eventType,
				input.tsWallMs,
				input.tsMonoMs,
				input.reason === "redaction_degraded" ? "{}" : toJson(input.payload),
				EVENT_PAYLOAD_DIGEST_VERSION,
				input.payloadDigest,
				input.reason,
				input.captureManifestFingerprint,
				input.now,
				input.now,
			);
		return { status: "quarantined", normalAck: false, receiptId, reason: input.reason };
	}

	private persistRawEventIdentityConflict(input: {
		repositoryIdentity: string;
		source: string;
		streamId: string;
		eventId: string;
		canonicalPayloadDigest: string;
		conflictingPayloadDigest: string;
		captureManifestFingerprint: string | null;
		now: string;
	}): RawEventCaptureOutcome {
		const receiptId = stableReceiptId(
			"conflict-receipt-v1",
			"free-mem:event-identity-conflict:v1",
			{
				repositoryIdentity: input.repositoryIdentity,
				source: input.source,
				streamId: input.streamId,
				eventId: input.eventId,
				payloadDigestVersion: EVENT_PAYLOAD_DIGEST_VERSION,
				canonicalPayloadDigest: input.canonicalPayloadDigest,
				conflictingPayloadDigest: input.conflictingPayloadDigest,
			},
		);
		this.db
			.prepare(
				`INSERT INTO raw_event_identity_conflicts(
					receipt_id, repository_identity, source, stream_id, event_id,
					payload_digest_version, canonical_payload_digest, conflicting_payload_digest,
					reason, receipt_state, canonical_unchanged, memory_delta,
					capture_manifest_fingerprint, first_seen_at, last_seen_at, occurrence_count
				 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'event_identity_payload_conflict',
					'non_success', 1, 0, ?, ?, ?, 1)
				 ON CONFLICT(receipt_id) DO UPDATE SET
					last_seen_at = excluded.last_seen_at,
					occurrence_count = raw_event_identity_conflicts.occurrence_count + 1`,
			)
			.run(
				receiptId,
				input.repositoryIdentity,
				input.source,
				input.streamId,
				input.eventId,
				EVENT_PAYLOAD_DIGEST_VERSION,
				input.canonicalPayloadDigest,
				input.conflictingPayloadDigest,
				input.captureManifestFingerprint,
				input.now,
				input.now,
			);
		return {
			status: "identity_conflict",
			normalAck: false,
			receiptId,
			reason: "event_identity_payload_conflict",
			canonicalUnchanged: true,
			memoryDelta: 0,
		};
	}

	private captureRawEvent(opts: RawEventCaptureInput): RawEventCaptureOutcome {
		if (!opts.opencodeSessionId.trim()) throw new Error("opencode_session_id is required");
		if (!opts.eventId.trim()) throw new Error("event_id is required");
		if (!opts.eventType.trim()) throw new Error("event_type is required");
		const [source, streamId] = this.normalizeStreamIdentity(
			opts.source ?? "opencode",
			opts.opencodeSessionId,
		);
		const repositoryIdentity = validatedRepositoryIdentity(opts.repositoryIdentity);
		const captureManifestFingerprint = validatedFingerprint(
			opts.captureManifestFingerprint,
			"capture_manifest_fingerprint",
		);
		const captureState = opts.captureState ?? (opts.redactionDegraded ? "quarantined" : "accepted");
		const degradedErrorCode = opts.redactionDegraded ? "redaction_degraded" : null;
		const safeErrorCode =
			captureState === "quarantined" ? (opts.safeErrorCode ?? degradedErrorCode) : null;
		if (captureState === "quarantined" && safeErrorCode !== "redaction_degraded") {
			throw new Error("redaction_degraded is required for quarantined capture");
		}
		const payload = opts.redactionDegraded ? {} : opts.payload;
		const storedPayload = captureState === "quarantined" ? {} : payload;
		const digest = rawEventPayloadDigest(payload);
		const sensitivity =
			captureState === "quarantined" ? "secret" : validatedSensitivity(opts.sensitivity);
		const now = nowIso();
		const existing = this.db
			.prepare(
				`SELECT id, event_seq, payload_digest, sensitivity, capture_state, safe_error_code
				 FROM raw_events
				 WHERE COALESCE(repository_identity, 'repo-v1:unknown') = ?
				   AND source = ? AND stream_id = ? AND event_id = ?`,
			)
			.get(
				repositoryIdentity ?? UNKNOWN_REPOSITORY_INDEX_SENTINEL,
				source,
				streamId,
				opts.eventId,
			) as ExistingRawEventRow | undefined;

		const reconcileExisting = (existing: ExistingRawEventRow): RawEventCaptureOutcome => {
			if (repositoryIdentity === null) {
				return this.persistRawEventQuarantine({
					repositoryIdentity,
					source,
					streamId,
					eventId: opts.eventId,
					eventType: opts.eventType,
					payload,
					payloadDigest: digest,
					tsWallMs: opts.tsWallMs ?? null,
					tsMonoMs: opts.tsMonoMs ?? null,
					captureManifestFingerprint,
					reason: "repository_identity_unknown_collision",
					now,
				});
			}
			if (existing.payload_digest !== digest) {
				return this.persistRawEventIdentityConflict({
					repositoryIdentity,
					source,
					streamId,
					eventId: opts.eventId,
					canonicalPayloadDigest: existing.payload_digest,
					conflictingPayloadDigest: digest,
					captureManifestFingerprint,
					now,
				});
			}
			const nextCaptureState =
				existing.capture_state === "quarantined" || captureState === "quarantined"
					? "quarantined"
					: "accepted";
			const nextSensitivity =
				nextCaptureState === "quarantined"
					? "secret"
					: strongestSensitivity(existing.sensitivity, sensitivity);
			const strengthened =
				nextSensitivity !== existing.sensitivity || nextCaptureState !== existing.capture_state;
			if (strengthened) {
				this.db
					.prepare(
						`UPDATE raw_events
						 SET payload_json = CASE WHEN ? = 'quarantined' THEN '{}' ELSE payload_json END,
							sensitivity = ?, capture_state = ?, safe_error_code = ?, created_at = created_at
						 WHERE id = ?`,
					)
					.run(
						nextCaptureState,
						nextSensitivity,
						nextCaptureState,
						nextCaptureState === "quarantined"
							? (existing.safe_error_code ?? safeErrorCode ?? "redaction_degraded")
							: null,
						existing.id,
					);
				this.db
					.prepare(
						`UPDATE memory_items
						 SET sensitivity = CASE
							WHEN sensitivity = 'secret' OR ? = 'secret' THEN 'secret'
							WHEN sensitivity = 'private' OR ? = 'private' THEN 'private'
							WHEN sensitivity = 'local_only' OR ? = 'local_only' THEN 'local_only'
							ELSE 'eligible'
							 END
							 WHERE repository_identity IS ?
							   AND source_event_ids_json IS NOT NULL
							   AND EXISTS (
								SELECT 1 FROM raw_event_flush_batches AS batch
								WHERE batch.attempt_fingerprint = memory_items.attempt_fingerprint
								  AND batch.source = ? AND batch.stream_id = ?
							   )
							   AND EXISTS (
								SELECT 1 FROM json_each(memory_items.source_event_ids_json)
								WHERE json_each.value = ?
							   )`,
					)
					.run(
						nextSensitivity,
						nextSensitivity,
						nextSensitivity,
						repositoryIdentity,
						source,
						streamId,
						opts.eventId,
					);
			}
			if (nextCaptureState === "quarantined") {
				return this.persistRawEventQuarantine({
					repositoryIdentity,
					source,
					streamId,
					eventId: opts.eventId,
					eventType: opts.eventType,
					payload,
					payloadDigest: digest,
					tsWallMs: opts.tsWallMs ?? null,
					tsMonoMs: opts.tsMonoMs ?? null,
					captureManifestFingerprint,
					reason: "redaction_degraded",
					now,
				});
			}
			return {
				status: "idempotent",
				normalAck: true,
				receiptId: stableReceiptId("event-receipt-v1", "free-mem:event-receipt:v1", {
					repositoryIdentity,
					source,
					streamId,
					eventId: opts.eventId,
					digest,
				}),
				eventSeq: Number(existing.event_seq),
			};
		};
		if (existing) return reconcileExisting(existing);

		this.d
			.insert(schema.rawEventSessions)
			.values({ opencode_session_id: streamId, source, stream_id: streamId, updated_at: now })
			.onConflictDoNothing()
			.run();
		const seqRow = this.d
			.update(schema.rawEventSessions)
			.set({
				last_received_event_seq: sql`${schema.rawEventSessions.last_received_event_seq} + 1`,
				updated_at: now,
			})
			.where(
				and(
					eq(schema.rawEventSessions.source, source),
					eq(schema.rawEventSessions.stream_id, streamId),
				),
			)
			.returning({ eventSeq: schema.rawEventSessions.last_received_event_seq })
			.get();
		if (!seqRow) throw new Error("Failed to allocate raw event seq");
		const eventSeq = Number(seqRow.eventSeq);
		this.d
			.insert(schema.rawEvents)
			.values({
				source,
				stream_id: streamId,
				opencode_session_id: streamId,
				event_id: opts.eventId,
				event_seq: eventSeq,
				event_type: opts.eventType,
				ts_wall_ms: opts.tsWallMs ?? null,
				ts_mono_ms: opts.tsMonoMs ?? null,
				payload_json: toJson(storedPayload),
				created_at: now,
				sensitivity,
				repository_identity: repositoryIdentity,
				capture_manifest_fingerprint: captureManifestFingerprint,
				capture_state: captureState,
				safe_error_code: safeErrorCode,
				payload_digest_version: EVENT_PAYLOAD_DIGEST_VERSION,
				payload_digest: digest,
			})
			.run();
		if (captureState === "quarantined") {
			return this.persistRawEventQuarantine({
				repositoryIdentity,
				source,
				streamId,
				eventId: opts.eventId,
				eventType: opts.eventType,
				payload,
				payloadDigest: digest,
				tsWallMs: opts.tsWallMs ?? null,
				tsMonoMs: opts.tsMonoMs ?? null,
				captureManifestFingerprint,
				reason: "redaction_degraded",
				now,
			});
		}
		return {
			status: "accepted",
			normalAck: true,
			receiptId: stableReceiptId("event-receipt-v1", "free-mem:event-receipt:v1", {
				repositoryIdentity,
				source,
				streamId,
				eventId: opts.eventId,
				digest,
			}),
			eventSeq,
		};
	}

	/** Persist one canonical event or one durable non-success receipt. */
	recordRawEvent(opts: RawEventCaptureInput): RawEventCaptureOutcome {
		return this.db.transaction(() => {
			const outcome = this.captureRawEvent(opts);
			this.updateRawEventIngestStats(
				outcome.status === "accepted" ? 1 : 0,
				0,
				outcome.status === "idempotent" ? 1 : 0,
				outcome.status === "identity_conflict" || outcome.status === "quarantined" ? 1 : 0,
			);
			return outcome;
		})();
	}

	/** Record a batch without dropping identity collisions before Store reconciliation. */
	recordRawEventsBatch(
		opencodeSessionId: string,
		events: Record<string, unknown>[],
		sourceName = "opencode",
	): { inserted: number; skipped: number; outcomes: RawEventCaptureOutcome[] } {
		if (!opencodeSessionId.trim()) throw new Error("opencode_session_id is required");
		return this.db.transaction(() => {
			let skippedInvalid = 0;
			let skippedDuplicate = 0;
			let skippedConflict = 0;
			const outcomes: RawEventCaptureOutcome[] = [];
			for (const event of events) {
				const captureInput = rawEventCaptureInputFromBatch(opencodeSessionId, sourceName, event);
				if (!captureInput) {
					skippedInvalid++;
					continue;
				}
				const outcome = this.captureRawEvent(captureInput);
				outcomes.push(outcome);
				if (outcome.status === "idempotent") skippedDuplicate++;
				else if (outcome.status === "identity_conflict" || outcome.status === "quarantined") {
					skippedConflict++;
				}
			}
			const inserted = outcomes.filter((outcome) => outcome.status === "accepted").length;
			this.updateRawEventIngestStats(inserted, skippedInvalid, skippedDuplicate, skippedConflict);
			return {
				inserted,
				skipped: skippedInvalid + skippedDuplicate + skippedConflict,
				outcomes,
			};
		})();
	}

	/**
	 * UPSERT session metadata (cwd, project, started_at, last_seen_ts_wall_ms).
	 * Port of update_raw_event_session_meta().
	 */
	updateRawEventSessionMeta(opts: {
		opencodeSessionId: string;
		source?: string;
		cwd?: string | null;
		project?: string | null;
		startedAt?: string | null;
		lastSeenTsWallMs?: number | null;
	}): void {
		const [source, streamId] = this.normalizeStreamIdentity(
			opts.source ?? "opencode",
			opts.opencodeSessionId,
		);
		const now = nowIso();
		const t = schema.rawEventSessions;
		this.d
			.insert(t)
			.values({
				opencode_session_id: streamId,
				source,
				stream_id: streamId,
				cwd: opts.cwd ?? null,
				project: opts.project ?? null,
				started_at: opts.startedAt ?? null,
				last_seen_ts_wall_ms: opts.lastSeenTsWallMs ?? null,
				updated_at: now,
			})
			.onConflictDoUpdate({
				target: [t.source, t.stream_id],
				set: {
					opencode_session_id: sql`excluded.opencode_session_id`,
					cwd: sql`COALESCE(excluded.cwd, ${t.cwd})`,
					project: sql`COALESCE(excluded.project, ${t.project})`,
					started_at: sql`COALESCE(excluded.started_at, ${t.started_at})`,
					last_seen_ts_wall_ms: sql`CASE
						WHEN excluded.last_seen_ts_wall_ms IS NULL THEN ${t.last_seen_ts_wall_ms}
						WHEN ${t.last_seen_ts_wall_ms} IS NULL THEN excluded.last_seen_ts_wall_ms
						WHEN excluded.last_seen_ts_wall_ms > ${t.last_seen_ts_wall_ms} THEN excluded.last_seen_ts_wall_ms
						ELSE ${t.last_seen_ts_wall_ms}
					END`,
					updated_at: sql`excluded.updated_at`,
				},
			})
			.run();
	}

	/**
	 * Get totals of pending (unflushed) raw events.
	 * Port of raw_event_backlog_totals().
	 */
	rawEventBacklogTotals(): { pending: number; sessions: number } {
		const row = this.d.get<{ sessions: number | null; pending: number | null }>(sql`
			WITH max_events AS (
				SELECT source, stream_id, MAX(event_seq) AS max_seq
				FROM raw_events
				GROUP BY source, stream_id
			)
			SELECT
				COUNT(1) AS sessions,
				SUM(e.max_seq - s.last_flushed_event_seq) AS pending
			FROM raw_event_sessions s
			JOIN max_events e ON e.source = s.source AND e.stream_id = s.stream_id
			WHERE e.max_seq > s.last_flushed_event_seq
		`);
		if (!row) return { sessions: 0, pending: 0 };
		return {
			sessions: Number(row.sessions ?? 0),
			pending: Number(row.pending ?? 0),
		};
	}

	/**
	 * Get the latest failed flush batch, or null if none.
	 * Port of latest_raw_event_flush_failure().
	 */
	latestRawEventFlushFailure(source?: string | null): Record<string, unknown> | null {
		const conditions = [
			inArray(schema.rawEventFlushBatches.status, ["error", "failed", "retry_exhausted"]),
		];
		if (source != null) {
			conditions.push(
				eq(schema.rawEventFlushBatches.source, source.trim().toLowerCase() || "opencode"),
			);
		}

		const row = this.d
			.select({
				id: schema.rawEventFlushBatches.id,
				source: schema.rawEventFlushBatches.source,
				stream_id: schema.rawEventFlushBatches.stream_id,
				opencode_session_id: schema.rawEventFlushBatches.opencode_session_id,
				start_event_seq: schema.rawEventFlushBatches.start_event_seq,
				end_event_seq: schema.rawEventFlushBatches.end_event_seq,
				extractor_version: schema.rawEventFlushBatches.extractor_version,
				status: schema.rawEventFlushBatches.status,
				updated_at: schema.rawEventFlushBatches.updated_at,
				attempt_count: schema.rawEventFlushBatches.attempt_count,
				error_message: schema.rawEventFlushBatches.error_message,
				error_type: schema.rawEventFlushBatches.error_type,
				observer_provider: schema.rawEventFlushBatches.observer_provider,
				observer_model: schema.rawEventFlushBatches.observer_model,
				observer_runtime: schema.rawEventFlushBatches.observer_runtime,
				observer_auth_source: schema.rawEventFlushBatches.observer_auth_source,
				observer_auth_type: schema.rawEventFlushBatches.observer_auth_type,
				observer_error_code: schema.rawEventFlushBatches.observer_error_code,
				observer_error_message: schema.rawEventFlushBatches.observer_error_message,
			})
			.from(schema.rawEventFlushBatches)
			.where(and(...conditions))
			.orderBy(desc(schema.rawEventFlushBatches.updated_at))
			.limit(1)
			.get();
		if (!row) return null;
		return { ...row, status: "error" };
	}

	sameActorPeerIds(): string[] {
		if (!tableExists(this.db, "sync_peers")) return [];
		const rows = this.d
			.select({ peer_device_id: schema.syncPeers.peer_device_id })
			.from(schema.syncPeers)
			.where(
				or(
					eq(schema.syncPeers.claimed_local_actor, 1),
					eq(schema.syncPeers.actor_id, this.actorId),
				),
			)
			.orderBy(schema.syncPeers.peer_device_id)
			.all();
		return rows.map((row) => String(row.peer_device_id ?? "").trim()).filter(Boolean);
	}

	// ref queries

	/** Find memories associated with a file path via the junction table index. */
	findByFile(
		filePath: string,
		options?: RefQueryOptions,
		destinationBoundary?: DestinationBoundaryV1,
	): RefQueryResult[] {
		return findByFileFn(this.db, filePath, options, destinationBoundary);
	}

	/** Find memories associated with a concept via the junction table index. */
	findByConcept(
		concept: string,
		options?: RefQueryOptions,
		destinationBoundary?: DestinationBoundaryV1,
	): RefQueryResult[] {
		return findByConceptFn(this.db, concept, options, destinationBoundary);
	}

	// close

	/** Close the database connection. */
	close(): void {
		this.db.pragma("optimize");
		if (this.ownsConnection) this.db.close();
	}
}
