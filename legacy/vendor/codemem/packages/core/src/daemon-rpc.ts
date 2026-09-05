import { createHash } from "node:crypto";
import type { Socket } from "node:net";
import { DaemonJobRequestError, type DaemonJobService } from "./daemon-jobs.js";
import type { DaemonCapabilityState } from "./daemon-lifecycle.js";
import { DaemonOperationRequestError, type DaemonOperationService } from "./daemon-operations.js";
import {
	type FileContextRetrievalAttempt,
	HOOK_DELIVERY_BUDGETS,
	LOCAL_API_VERSION,
	RPC_CAPABILITY_HASH,
	RPC_DEFAULT_DEADLINE_MS,
	RPC_MAX_BYTES,
	RPC_METHODS,
	type RpcMethod,
	type RpcRequest,
	type RpcSuccess,
	rpcDeadlineForMethod,
	type TypedRpcError,
	typedError,
} from "./daemon-rpc-contract.js";
import { isEmbeddingDisabled } from "./db.js";
import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileUntrustedDestinationBoundary,
	type DestinationBoundaryV1,
	type DestinationConsumerV1,
} from "./destination-boundary.js";
import { validateMemoryKind } from "./memory-kinds.js";
import {
	dispatchClassA,
	hashMutationPayload,
	MutationConflictError,
} from "./mutation-dispatcher.js";
import {
	NORMALIZED_EVENT_FIELDS,
	NORMALIZED_SCHEMA_VERSION,
	sealDegradedNormalizedEvent,
	validateNormalizedEvent,
} from "./normalized-event.js";
import { collectOperationalStatus } from "./operational-status.js";
import { resolveRepositoryIdentity } from "./project.js";

export { NORMALIZED_SCHEMA_VERSION } from "./normalized-event.js";

import {
	BackupRequestError,
	listCanonicalBackups,
	verifyCanonicalBackup,
} from "./online-backup.js";
import { applyDaemonIntake, type RedactionResult } from "./redaction-pipeline.js";
import { REDACTION_WORKER_DEADLINE_MS } from "./redaction-worker.js";
import {
	type RetrievalStatus,
	type RetrievalSurface,
	tryUpdateRetrievalDelivery,
} from "./retrieval-ledger.js";
import {
	reconcileFailedRetrievalSurface,
	recordRetrievalSurface,
	resolveRetrievalSession,
} from "./retrieval-surface-ledger.js";
import {
	readSpoolStatus,
	type SpoolEntry,
	type SpoolImportOutcome,
	type SpoolRedactionMetadata,
	validateSpoolRedaction,
} from "./spool.js";
import { type MemoryStore, ProcessingResumeError } from "./store.js";
import type { DoctorProcessingJobProjection, MemoryFilters, SensitivityV1 } from "./types.js";
import type { ViewerAuthState } from "./viewer-auth.js";
import type { ViewerReadHandler } from "./viewer-read.js";
import type { WriterActor } from "./writer-actor.js";

type RpcIdentity = { pid: number; nonce: string };
const RECENT_FAILURE_WINDOW_MS = 24 * 60 * 60 * 1000;
export const CAPTURE_CONCURRENCY_LIMIT = 2;

function spoolHealth(dataDir: string): Record<string, unknown> {
	try {
		const status = readSpoolStatus(dataDir);
		return {
			status: status.critical ? "critical" : status.warnings.length > 0 ? "warning" : "ok",
			critical: status.critical,
			warnings: status.warnings,
			droppedTotal: status.dropped.total,
			droppedByKind: status.dropped.byKind,
			firstDroppedAt: status.dropped.firstDroppedAt,
			lastDroppedAt: status.dropped.lastDroppedAt,
			quarantineRejected: status.dropped.quarantineRejected,
			normalBytes: status.normalBytes,
			reservedBytes: status.reservedBytes,
			quarantineBytes: status.quarantineBytes,
		};
	} catch {
		return {
			status: "unavailable",
			critical: true,
			warnings: ["spool status unavailable"],
		};
	}
}

export type {
	FileContextRetrievalAttempt,
	RpcMethod,
	RpcRequest,
	RpcSuccess,
	TypedRpcError,
} from "./daemon-rpc-contract.js";
export {
	callDaemonRpc,
	HOOK_DELIVERY_BUDGETS,
	LOCAL_API_VERSION,
	mapPeerConnectError,
	RPC_CAPABILITY_HASH,
	RPC_DEFAULT_DEADLINE_MS,
	RPC_MAX_BYTES,
	RPC_METHODS,
	rpcDeadlineForMethod,
} from "./daemon-rpc-contract.js";

const TOP_LEVEL_FIELDS = new Set([
	"id",
	"method",
	"adapter_version",
	"native_cli_version",
	"normalized_schema_version",
	"local_api_version",
	"capability_hash",
	"body",
]);

const METHOD_BODY_FIELDS: Record<RpcMethod, readonly string[]> = {
	"GET /v1/health": [],
	"GET /v1/doctor": [],
	"POST /v1/events": ["idempotencyKey", "event", "adapterRedaction"],
	"POST /v1/events/batch": ["items"],
	"POST /v1/context/pack": ["requestId", "context", "limit", "tokenBudget", "filters", "trace"],
	"POST /v1/search": [
		"requestId",
		"mode",
		"query",
		"repositoryPath",
		"ids",
		"memoryId",
		"depthBefore",
		"depthAfter",
		"includePackContext",
		"filters",
		"limit",
	],
	"POST /v1/retrieval/file-context": [
		"attemptId",
		"startedAt",
		"completedAt",
		"retrievalStatus",
		"candidateIds",
		"candidateCount",
		"selectedIds",
		"failureCode",
		"failureStage",
		"project",
		"repositoryPath",
		"sourceSessionId",
	],
	"POST /v1/retrieval/file-context/delivery": ["attemptId", "status"],
	"GET /v1/memories/:id": ["id", "requestId", "project", "kind"],
	"POST /v1/memories/record": [
		"idempotencyKey",
		"kind",
		"title",
		"body",
		"confidence",
		"project",
		"adapterRedaction",
		// Probe input for server-side repository-identity resolution; the caller
		// never supplies the identity itself.
		"cwd",
	],
	"DELETE /v1/memories/:id": ["id", "requestId", "expectedRevision"],
	"GET /v1/checkpoints": ["project", "state", "limit"],
	"GET /v1/view": ["collection", "sessionId", "project", "kind", "scope", "limit", "offset"],
	"POST /v1/viewer/auth/nonce": [],
	"POST /v1/viewer/auth/exchange": ["nonce"],
	"POST /v1/viewer/auth/verify": ["bearer", "session"],
	"POST /v1/viewer/auth/logout": ["session"],
	"GET /v1/backup/list": [],
	"POST /v1/backup/create": ["operationId", "payloadHash", "reason"],
	"POST /v1/backup/verify": ["backupId"],
	"POST /v1/backup/restore": ["operationId", "payloadHash", "backupId"],
	"POST /v1/operations/export": ["operationId", "payloadHash", "outputPath", "filters"],
	"POST /v1/operations/import": [
		"operationId",
		"payloadHash",
		"inputPath",
		"remapProject",
		"dryRun",
	],
	"GET /v1/operations/:id": ["id"],
	"POST /v1/jobs": ["kind", "args", "dryRun"],
	"GET /v1/jobs": ["kind", "state", "submittedAfter"],
	"GET /v1/jobs/:id": ["id"],
	"GET /v1/processing-jobs/:id": ["id"],
	"POST /v1/processing-jobs/:id/doctor-retry": [
		"id",
		"producerReceiptId",
		"expectedRole",
		"expectedProviderFingerprint",
		"expectedManifestFingerprint",
		"expectedAttemptCount",
		"expectedClaimGeneration",
	],
};

const METHOD_REQUIRED_FIELDS: Record<RpcMethod, readonly string[]> = {
	"GET /v1/health": [],
	"GET /v1/doctor": [],
	"POST /v1/events": ["idempotencyKey", "event"],
	"POST /v1/events/batch": ["items"],
	"POST /v1/context/pack": ["requestId", "context"],
	"POST /v1/search": ["requestId", "mode"],
	"POST /v1/retrieval/file-context": ["attemptId", "startedAt", "completedAt", "retrievalStatus"],
	"POST /v1/retrieval/file-context/delivery": ["attemptId", "status"],
	"GET /v1/memories/:id": ["id", "requestId"],
	"POST /v1/memories/record": ["idempotencyKey", "kind", "title", "body"],
	"DELETE /v1/memories/:id": ["id", "requestId"],
	"GET /v1/checkpoints": [],
	"GET /v1/view": ["collection"],
	"POST /v1/viewer/auth/nonce": [],
	"POST /v1/viewer/auth/exchange": ["nonce"],
	"POST /v1/viewer/auth/verify": [],
	"POST /v1/viewer/auth/logout": ["session"],
	"GET /v1/backup/list": [],
	"POST /v1/backup/create": ["operationId", "payloadHash", "reason"],
	"POST /v1/backup/verify": ["backupId"],
	"POST /v1/backup/restore": ["operationId", "payloadHash", "backupId"],
	"POST /v1/operations/export": ["operationId", "payloadHash", "outputPath", "filters"],
	"POST /v1/operations/import": ["operationId", "payloadHash", "inputPath"],
	"GET /v1/operations/:id": ["id"],
	"POST /v1/jobs": ["kind"],
	"GET /v1/jobs": [],
	"GET /v1/jobs/:id": ["id"],
	"GET /v1/processing-jobs/:id": ["id"],
	"POST /v1/processing-jobs/:id/doctor-retry": [
		"id",
		"producerReceiptId",
		"expectedRole",
		"expectedProviderFingerprint",
		"expectedManifestFingerprint",
		"expectedAttemptCount",
		"expectedClaimGeneration",
	],
};

const MAINTENANCE_BLOCKED_METHODS = new Set<RpcMethod>([
	"POST /v1/events",
	"POST /v1/events/batch",
	"POST /v1/context/pack",
	"POST /v1/search",
	"POST /v1/retrieval/file-context",
	"POST /v1/retrieval/file-context/delivery",
	"GET /v1/memories/:id",
	"POST /v1/memories/record",
	"DELETE /v1/memories/:id",
	"POST /v1/backup/create",
	"POST /v1/backup/restore",
	"POST /v1/operations/export",
	"POST /v1/operations/import",
	"POST /v1/jobs",
	"POST /v1/processing-jobs/:id/doctor-retry",
]);

const VIEW_COLLECTIONS = new Set([
	"sessions",
	"projects",
	"memories",
	"observations",
	"summaries",
	"session",
	"memory",
	"artifacts",
	"raw-events",
	"raw-events-status",
	"stats",
	"runtime",
	"usage",
	"observer-status",
	"config",
]);

const PERSISTED_ID_FIELDS = new Set(["idempotencyKey", "requestId", "operationId", "backupId"]);

const FILTER_FIELDS = new Set([
	"kind",
	"session_id",
	"since",
	"working_set_paths",
	"project",
	"scope_id",
	"include_scope_ids",
	"exclude_scope_ids",
	"visibility",
	"include_visibility",
	"exclude_visibility",
	"include_workspace_ids",
	"exclude_workspace_ids",
	"include_workspace_kinds",
	"exclude_workspace_kinds",
	"include_actor_ids",
	"exclude_actor_ids",
	"include_trust_states",
	"exclude_trust_states",
	"ownership_scope",
	"personal_first",
	"trust_bias",
	"widen_shared_when_weak",
	"widen_shared_min_personal_results",
	"widen_shared_min_personal_score",
	"widen_project_when_weak",
	"widen_project_min_results",
	"widen_project_min_score",
	"widen_project_max_results",
]);
const STRING_FILTER_FIELDS = new Set(["kind", "since", "project", "ownership_scope", "trust_bias"]);
const STRING_ARRAY_FILTER_FIELDS = new Set([
	"working_set_paths",
	"include_scope_ids",
	"exclude_scope_ids",
	"include_visibility",
	"exclude_visibility",
	"include_workspace_ids",
	"exclude_workspace_ids",
	"include_workspace_kinds",
	"exclude_workspace_kinds",
	"include_actor_ids",
	"exclude_actor_ids",
	"include_trust_states",
	"exclude_trust_states",
]);
const STRING_OR_ARRAY_FILTER_FIELDS = new Set(["scope_id", "visibility"]);
const BOOLEAN_OR_STRING_FILTER_FIELDS = new Set([
	"personal_first",
	"widen_shared_when_weak",
	"widen_project_when_weak",
]);
const NUMBER_FILTER_FIELDS = new Set([
	"widen_shared_min_personal_results",
	"widen_shared_min_personal_score",
	"widen_project_min_results",
	"widen_project_min_score",
	"widen_project_max_results",
]);

class RpcRequestError extends Error {
	constructor(
		message: string,
		readonly code = "invalid_request",
		readonly retryable = false,
	) {
		super(message);
		this.name = "RpcRequestError";
	}
}

export type ProtocolVersions = {
	localApi: number;
	normalizedSchema: number;
};

export type DaemonRpcContext = {
	identity: RpcIdentity;
	dataDir: string;
	deadlineMs?: number;
	now?: () => number;
	onStop: () => void;
	writer: WriterActor;
	store: MemoryStore;
	viewerAuth: ViewerAuthState;
	viewerRead: ViewerReadHandler;
	jobs: DaemonJobService;
	operations: DaemonOperationService;
	capability: DaemonCapabilityState;
	restoreState?: { active: boolean };
	captureInFlight?: number;
};

function isMaintenanceMode(ctx: DaemonRpcContext): boolean {
	return ctx.jobs.isMaintenanceMode() || ctx.restoreState?.active === true;
}

function daemonReadBoundary(
	ctx: DaemonRpcContext,
	consumer: DestinationConsumerV1,
): DestinationBoundaryV1 {
	return compileUntrustedDestinationBoundary({
		consumer,
		configurationFingerprint:
			ctx.capability.configurationFingerprint ?? CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	});
}

function processingJobForDoctor(
	ctx: DaemonRpcContext,
	jobId: number,
): DoctorProcessingJobProjection | null {
	const job = ctx.store.getProcessingJobForDoctor(jobId);
	if (!job) return null;
	const retryTarget =
		ctx.capability.mode === "configured"
			? {
					manifestFingerprint: ctx.capability.configurationFingerprint,
					providerFingerprint: ctx.capability.summaryProvider.providerFingerprint,
				}
			: null;
	let nextAction: DoctorProcessingJobProjection["nextAction"] = "none";
	if (job.state === "retry_exhausted") {
		nextAction = retryTarget ? "confirm_retry" : "activate_valid_manifest";
	}
	return {
		...job,
		retryTarget,
		nextAction,
	};
}

function isSafePersistedText(value: unknown, maxBytes: number): value is string {
	if (
		typeof value !== "string" ||
		value.length === 0 ||
		Buffer.byteLength(value, "utf8") > maxBytes
	) {
		return false;
	}
	const intake = applyDaemonIntake({ id: value }, { allowlist: ["id"] });
	return !intake.degraded && intake.sensitivity === "normal" && intake.payload.id === value;
}

function unknownFields(
	value: Record<string, unknown>,
	allowed: ReadonlySet<string> | readonly string[],
): string[] {
	const allow = allowed instanceof Set ? allowed : new Set(allowed);
	return Object.keys(value).filter((key) => !allow.has(key));
}

function parseRequest(raw: string): RpcRequest | TypedRpcError {
	let parsed: unknown;
	try {
		parsed = JSON.parse(raw);
	} catch {
		return typedError("invalid_json", "RPC request is not valid JSON.");
	}
	if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
		return typedError("invalid_json", "RPC request must be a JSON object.");
	}
	const object = parsed as Record<string, unknown>;
	const extra = unknownFields(object, TOP_LEVEL_FIELDS);
	if (extra.length > 0) {
		return typedError("unknown_field", `Unknown RPC field: ${extra[0]}`);
	}
	if (typeof object.id !== "string" || object.id.length === 0) {
		return typedError("invalid_request", "RPC request id is required.");
	}
	if (typeof object.method !== "string") {
		return typedError("unknown_method", "RPC method is required.");
	}
	if (
		typeof object.adapter_version !== "string" ||
		typeof object.native_cli_version !== "string" ||
		typeof object.normalized_schema_version !== "number" ||
		typeof object.local_api_version !== "number" ||
		typeof object.capability_hash !== "string"
	) {
		return typedError("protocol_mismatch", "RPC handshake fields are missing or mistyped.");
	}
	if (
		object.body !== undefined &&
		(typeof object.body !== "object" || object.body === null || Array.isArray(object.body))
	) {
		return typedError("invalid_request", "RPC body must be an object.");
	}
	return object as RpcRequest;
}

function handshakeError(request: RpcRequest): TypedRpcError | null {
	if (request.local_api_version !== LOCAL_API_VERSION) {
		return typedError("protocol_mismatch", "local_api_version is incompatible.");
	}
	if (request.normalized_schema_version !== NORMALIZED_SCHEMA_VERSION) {
		return typedError("protocol_mismatch", "normalized_schema_version is incompatible.");
	}
	if (request.capability_hash !== RPC_CAPABILITY_HASH) {
		return typedError("protocol_mismatch", "capability_hash is incompatible.");
	}
	return null;
}

function isRpcMethod(method: string): method is RpcMethod {
	return (RPC_METHODS as readonly string[]).includes(method);
}

function protocolVersions(): ProtocolVersions {
	return { localApi: LOCAL_API_VERSION, normalizedSchema: NORMALIZED_SCHEMA_VERSION };
}

async function handleMethod(
	method: RpcMethod,
	body: Record<string, unknown>,
	ctx: DaemonRpcContext,
	normalizedSchemaVersion: number,
	nativeCliVersion: string,
): Promise<Record<string, unknown>> {
	if (method === "GET /v1/health") {
		return {
			status: "ok",
			instanceId: ctx.identity.nonce,
			maintenanceMode: isMaintenanceMode(ctx),
			protocolVersion: protocolVersions(),
			spool: spoolHealth(ctx.dataDir),
			capability: ctx.capability,
		};
	}
	if (method === "GET /v1/doctor") {
		const operationalStatus = collectOperationalStatus(ctx.writer, {
			embeddingDisabled: isEmbeddingDisabled(),
			recentFailureCutoff: new Date(Date.now() - RECENT_FAILURE_WINDOW_MS).toISOString(),
			capability: ctx.capability,
		});
		const degradedDeliveries = Number(
			(
				ctx.writer
					.prepare(
						`SELECT
						(SELECT COUNT(*) FROM raw_events
							 WHERE safe_error_code = 'redaction_degraded'
							    OR json_extract(payload_json, '$.redaction_degraded') = 1) +
						(SELECT COUNT(*) FROM memory_items
							 WHERE json_extract(metadata_json, '$.redaction_degraded') = 1) +
						(SELECT COUNT(*) FROM daemon_jobs
							 WHERE error_code = 'redaction_degraded') AS count`,
					)
					.get() as { count?: number } | undefined
			)?.count ?? 0,
		);
		return {
			status: "ok",
			instanceId: ctx.identity.nonce,
			maintenanceMode: isMaintenanceMode(ctx),
			protocolVersion: protocolVersions(),
			diagnostics: {
				pid: ctx.identity.pid,
				dataDir: ctx.dataDir,
				lock: "held",
				socket: "listening",
				platform: process.platform,
				spool: spoolHealth(ctx.dataDir),
				hookDelivery: {
					implementation: "node-fallback",
					p95TargetMs: 150,
					budgets: HOOK_DELIVERY_BUDGETS,
				},
				redaction: {
					status: degradedDeliveries > 0 ? "warning" : "ok",
					degradedDeliveries,
					workerDeadlineMs: REDACTION_WORKER_DEADLINE_MS,
				},
				operationalStatus,
				capability: ctx.capability,
			},
		};
	}
	if (method === "GET /v1/backup/list") {
		return { backups: listCanonicalBackups(ctx.dataDir) };
	}
	if (method === "POST /v1/backup/create") {
		return ctx.operations.runBackup("backup-create", body);
	}
	if (method === "POST /v1/backup/verify") {
		const check = verifyCanonicalBackup({
			dataDir: ctx.dataDir,
			backupId: String(body.backupId),
		});
		return {
			backupId: check.backupId,
			valid: check.valid,
			manifestHash: check.manifestHash,
			diagnostics: check.diagnostics,
		};
	}
	if (method === "POST /v1/backup/restore") {
		const restoreState = ctx.restoreState ?? { active: false };
		ctx.restoreState = restoreState;
		if (restoreState.active || ctx.operations.hasPending() || ctx.jobs.hasPendingWork()) {
			throw new BackupRequestError("conflict", "Restore conflicts with active daemon work.");
		}
		restoreState.active = true;
		try {
			return await ctx.operations.runBackup("backup-restore", body);
		} catch (error) {
			restoreState.active = false;
			throw error;
		}
	}
	if (method === "POST /v1/events" || method === "POST /v1/memories/record") {
		let adapterRedaction: SpoolRedactionMetadata | undefined;
		try {
			adapterRedaction =
				body.adapterRedaction === undefined
					? undefined
					: validateSpoolRedaction(body.adapterRedaction);
		} catch {
			throw new RpcRequestError("adapterRedaction is malformed.");
		}
		return method === "POST /v1/events"
			? withCaptureAdmissionLease(ctx, () =>
					persistEvent(ctx, body, normalizedSchemaVersion, adapterRedaction),
				)
			: handleRemember(ctx, body, adapterRedaction);
	}
	if (method === "POST /v1/events/batch") {
		return withCaptureAdmissionLease(ctx, () =>
			handleEventBatch(ctx, body, normalizedSchemaVersion),
		);
	}
	if (method === "DELETE /v1/memories/:id") {
		return handleForget(ctx, body);
	}
	if (method === "POST /v1/context/pack") {
		return handleRetrievalRpc(ctx, body, mcpRetrievalSurface(method, body, nativeCliVersion), () =>
			handlePack(ctx, body),
		);
	}
	if (method === "POST /v1/search") {
		return handleRetrievalRpc(ctx, body, mcpRetrievalSurface(method, body, nativeCliVersion), () =>
			handleSearch(ctx, body),
		);
	}
	if (method === "POST /v1/retrieval/file-context") {
		return handleFileContextRetrieval(ctx, body as FileContextRetrievalAttempt);
	}
	if (method === "POST /v1/retrieval/file-context/delivery") {
		return handleFileContextDelivery(ctx, body);
	}
	if (method === "GET /v1/memories/:id") {
		return handleRetrievalRpc(ctx, body, mcpRetrievalSurface(method, body, nativeCliVersion), () =>
			handleMemoryGet(ctx, body),
		);
	}
	if (method === "GET /v1/checkpoints") {
		return { checkpoints: [] };
	}
	if (method === "GET /v1/view") {
		return handleView(ctx, body);
	}
	if (method === "POST /v1/viewer/auth/nonce") {
		return ctx.viewerAuth.issueNonce();
	}
	if (method === "POST /v1/viewer/auth/exchange") {
		if (typeof body.nonce !== "string") throw new RpcRequestError("nonce must be a string.");
		return { session: ctx.viewerAuth.exchangeNonce(body.nonce) };
	}
	if (method === "POST /v1/viewer/auth/verify") {
		if (body.bearer !== undefined && typeof body.bearer !== "string") {
			throw new RpcRequestError("bearer must be a string.");
		}
		if (body.session !== undefined && typeof body.session !== "string") {
			throw new RpcRequestError("session must be a string.");
		}
		const bearer = typeof body.bearer === "string" ? body.bearer : null;
		const session = typeof body.session === "string" ? body.session : null;
		return {
			authenticated:
				(bearer !== null && ctx.viewerAuth.verifyBearer(bearer)) ||
				(session !== null && ctx.viewerAuth.verifySession(session)),
		};
	}
	if (method === "POST /v1/viewer/auth/logout") {
		if (typeof body.session !== "string") throw new RpcRequestError("session must be a string.");
		return { loggedOut: ctx.viewerAuth.logout(body.session) };
	}
	if (method === "POST /v1/operations/export") {
		return ctx.operations.submit("export", body);
	}
	if (method === "POST /v1/operations/import") {
		return ctx.operations.submit("import", body);
	}
	if (method === "GET /v1/operations/:id") {
		return ctx.operations.get(body.id);
	}
	if (method === "POST /v1/jobs") {
		try {
			return ctx.jobs.submit({ kind: body.kind, args: body.args, dryRun: body.dryRun });
		} catch (error) {
			if (error instanceof DaemonJobRequestError) throw new RpcRequestError(error.message);
			throw error;
		}
	}
	if (method === "GET /v1/jobs") {
		try {
			return {
				jobs: ctx.jobs.list({
					kind: body.kind,
					state: body.state,
					submittedAfter: body.submittedAfter,
				}),
			};
		} catch (error) {
			if (error instanceof DaemonJobRequestError) throw new RpcRequestError(error.message);
			throw error;
		}
	}
	if (method === "GET /v1/jobs/:id") {
		try {
			return { job: ctx.jobs.get(body.id) };
		} catch (error) {
			if (error instanceof DaemonJobRequestError) throw new RpcRequestError(error.message);
			throw error;
		}
	}
	if (method === "GET /v1/processing-jobs/:id") {
		const job = processingJobForDoctor(ctx, requirePositiveInt(body.id));
		if (!job) throw new RpcRequestError("Processing job was not found.", "not_found");
		return { job };
	}
	if (method === "POST /v1/processing-jobs/:id/doctor-retry") {
		if (body.expectedRole !== "summary") {
			throw new RpcRequestError("expectedRole must be summary.");
		}
		const expectedProviderFingerprint = body.expectedProviderFingerprint;
		const expectedManifestFingerprint = body.expectedManifestFingerprint;
		if (
			typeof body.producerReceiptId !== "string" ||
			!Object.hasOwn(body, "expectedProviderFingerprint") ||
			!Object.hasOwn(body, "expectedManifestFingerprint") ||
			!(
				(typeof expectedProviderFingerprint === "string" &&
					typeof expectedManifestFingerprint === "string") ||
				(expectedProviderFingerprint === null && expectedManifestFingerprint === null)
			) ||
			!Number.isSafeInteger(body.expectedAttemptCount) ||
			!Number.isSafeInteger(body.expectedClaimGeneration)
		) {
			throw new RpcRequestError("Doctor retry snapshot is invalid.");
		}
		const jobId = requirePositiveInt(body.id);
		const job = processingJobForDoctor(ctx, jobId);
		return ctx.store.confirmDoctorRetry({
			jobId,
			producerReceiptId: body.producerReceiptId,
			expectedRole: "summary",
			expectedProviderFingerprint,
			expectedManifestFingerprint,
			expectedAttemptCount: body.expectedAttemptCount as number,
			expectedClaimGeneration: body.expectedClaimGeneration as number,
			targetProviderFingerprint: job?.retryTarget?.providerFingerprint ?? null,
			targetManifestFingerprint: job?.retryTarget?.manifestFingerprint ?? null,
		});
	}
	return {
		operationId: body.operationId ?? body.id,
		state: "not_implemented",
	};
}

function strongestSensitivity(
	claimed: SpoolRedactionMetadata["sensitivity"],
	detected: SpoolRedactionMetadata["sensitivity"],
): SpoolRedactionMetadata["sensitivity"] {
	if (claimed === "secret" || detected === "secret") return "secret";
	if (claimed === "private" || detected === "private") return "private";
	return "normal";
}

function mergedRedaction(
	intake: RedactionResult,
	previous?: SpoolRedactionMetadata,
	claimed: SpoolRedactionMetadata["sensitivity"] = "normal",
): SpoolRedactionMetadata {
	const versions = [
		...new Set([intake.secret_rules_version, ...(previous?.secret_rules_version.split("+") ?? [])]),
	].sort();
	return {
		sensitivity: strongestSensitivity(
			strongestSensitivity(claimed, intake.sensitivity),
			previous?.sensitivity ?? "normal",
		),
		secret_rules_version: versions.join("+"),
		redaction_degraded: intake.degraded || (previous?.redaction_degraded ?? false),
		private_content_omitted:
			intake.private_content_omitted || (previous?.private_content_omitted ?? false),
		local_only: intake.local_only || (previous?.local_only ?? false),
	};
}

function normalizedEvent(
	body: Record<string, unknown>,
	normalizedSchemaVersion: number,
	previousRedaction?: SpoolRedactionMetadata,
): Record<string, unknown> {
	const event = asObject(body.event);
	try {
		validateNormalizedEvent(event, normalizedSchemaVersion);
	} catch (error) {
		throw new RpcRequestError(error instanceof Error ? error.message : "event is invalid.");
	}
	const intake = applyDaemonIntake(event, {
		allowlist: [...NORMALIZED_EVENT_FIELDS],
		metadataKeys: NORMALIZED_EVENT_FIELDS.filter((field) => field !== "payload"),
	});
	const payload =
		intake.degraded || previousRedaction?.redaction_degraded
			? sealDegradedNormalizedEvent(intake.payload)
			: intake.payload;
	if (!Object.hasOwn(payload, "payload")) payload.payload = {};
	let validated: ReturnType<typeof validateNormalizedEvent>;
	try {
		validated = validateNormalizedEvent(payload, normalizedSchemaVersion);
	} catch (error) {
		throw new RpcRequestError(error instanceof Error ? error.message : "event is invalid.");
	}
	const { eventId, idempotencyKey, sensitivity } = validated;
	if (idempotencyKey !== body.idempotencyKey) {
		throw new RpcRequestError("event.idempotencyKey must match the request envelope.");
	}
	const redaction = mergedRedaction(intake, previousRedaction, sensitivity);
	if (redaction.sensitivity === "secret") payload.payload = {};
	return {
		...payload,
		eventId,
		sensitivity: redaction.sensitivity,
		secret_rules_version: redaction.secret_rules_version,
		redaction_degraded: redaction.redaction_degraded,
		private_content_omitted: redaction.private_content_omitted,
		local_only: redaction.local_only,
	};
}

function persistEvent(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
	normalizedSchemaVersion: number,
	previousRedaction?: SpoolRedactionMetadata,
): Record<string, unknown> {
	return persistEventWithReceipt(ctx, body, normalizedSchemaVersion, previousRedaction).result;
}

function persistEventWithReceipt(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
	normalizedSchemaVersion: number,
	previousRedaction?: SpoolRedactionMetadata,
): { result: Record<string, unknown>; incomingPayloadHash: string; receiptPayloadHash: string } {
	const event = normalizedEvent(body, normalizedSchemaVersion, previousRedaction);
	const eventId = String(event.eventId);
	const eventType = String(event.kind);
	const nativeSessionId = String(event.nativeSessionId);
	const source = event.agent === "claude-code" ? "claude" : String(event.agent);
	const occurredAtMs = Date.parse(String(event.occurredAt));
	const redactionDegraded = event.redaction_degraded === true;
	const repositoryIdentity = resolveRepositoryIdentity(String(event.cwd));
	let captureSensitivity = "eligible";
	if (redactionDegraded || event.sensitivity === "secret") captureSensitivity = "secret";
	else if (event.sensitivity === "private") captureSensitivity = "private";
	else if (event.local_only === true) captureSensitivity = "local_only";
	const { payload, ...normalizedMetadata } = event;
	const { idempotencyKey: _idempotencyKey, ...eventWithoutIdempotencyKey } = event;
	const rawPayload =
		payload && typeof payload === "object" && !Array.isArray(payload)
			? {
					...(payload as Record<string, unknown>),
					...eventWithoutIdempotencyKey,
					_normalized: normalizedMetadata,
				}
			: { ...eventWithoutIdempotencyKey, _normalized: normalizedMetadata };
	const persist = () => {
		const recorded = ctx.store.recordRawEventsBatch(
			nativeSessionId,
			[
				{
					event_id: eventId,
					event_type: eventType,
					payload: rawPayload,
					ts_wall_ms: occurredAtMs,
					repository_identity: repositoryIdentity,
					capture_manifest_fingerprint: ctx.capability.configurationFingerprint,
					sensitivity: captureSensitivity,
					capture_state: redactionDegraded ? "quarantined" : "accepted",
					safe_error_code: redactionDegraded ? "redaction_degraded" : null,
					redaction_degraded: redactionDegraded,
				},
			],
			source,
		);
		const outcome = recorded.outcomes[0];
		if (!outcome) throw new Error("event was not persisted.");
		if (outcome.normalAck) {
			ctx.store.updateRawEventSessionMeta({
				opencodeSessionId: nativeSessionId,
				source,
				cwd: String(event.cwd),
				project: String(event.projectKey),
				startedAt: eventType === "session_started" ? String(event.occurredAt) : null,
				lastSeenTsWallMs: occurredAtMs,
			});
			return { eventId, receiptId: outcome.receiptId, status: "committed" };
		}
		return outcome.status === "identity_conflict"
			? {
					eventId,
					receiptId: outcome.receiptId,
					status: "conflict",
					canonicalUnchanged: outcome.canonicalUnchanged,
					memoryDelta: outcome.memoryDelta,
				}
			: {
					eventId,
					receiptId: outcome.receiptId,
					status: "quarantined",
					safeErrorCode: outcome.reason,
				};
	};
	const mutationPayload = { event };
	const receipt = dispatchClassA(ctx.writer, {
		method: "POST /v1/events",
		idempotencyKey: String(body.idempotencyKey),
		payload: mutationPayload,
		apply: persist,
		reconcileExisting: (existing) =>
			existing.result.eventId === eventId ? { ...existing, result: persist() } : null,
	});
	const { eventId: _eventId, ...result } = receipt.result;
	return {
		result,
		incomingPayloadHash: hashMutationPayload(mutationPayload),
		receiptPayloadHash: receipt.payloadHash,
	};
}

function assertCaptureCapacity(ctx: DaemonRpcContext): void {
	if ((ctx.captureInFlight ?? 0) >= CAPTURE_CONCURRENCY_LIMIT) {
		throw new RpcRequestError(
			"Capture admission is saturated; retry through the spool.",
			"capture_saturated",
			true,
		);
	}
}

async function withCaptureAdmissionLease<T>(ctx: DaemonRpcContext, persist: () => T): Promise<T> {
	assertCaptureCapacity(ctx);
	ctx.captureInFlight = (ctx.captureInFlight ?? 0) + 1;
	try {
		const result = persist();
		// Keep the lease through response scheduling so sibling socket arrivals
		// contend for the two slots without delaying persistence.
		await new Promise<void>((resolve) => setImmediate(resolve));
		return result;
	} finally {
		ctx.captureInFlight = Math.max(0, (ctx.captureInFlight ?? 1) - 1);
	}
}

function handleEventBatch(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
	normalizedSchemaVersion: number,
): Record<string, unknown> {
	if (!Array.isArray(body.items)) {
		throw new RpcRequestError("items must be an array.");
	}
	if (body.items.length === 0 || body.items.length > 200) {
		throw new RpcRequestError("items must contain between 1 and 200 entries.");
	}
	const rows = body.items.map((item) => {
		const row = asObject(item);
		const extra = unknownFields(row, ["idempotencyKey", "event"]);
		if (extra.length > 0) {
			throw new RpcRequestError(`Unknown batch item field: ${extra[0]}`);
		}
		if (typeof row.idempotencyKey !== "string" || row.idempotencyKey.length === 0) {
			throw new RpcRequestError("idempotencyKey is required for every batch item.");
		}
		if (!isSafePersistedText(row.idempotencyKey, 256)) {
			throw new RpcRequestError("idempotencyKey is invalid for a batch item.");
		}
		asObject(row.event);
		return row;
	});
	const receipts: Record<string, unknown>[] = [];
	for (const row of rows) {
		try {
			receipts.push(
				persistEvent(
					ctx,
					{ idempotencyKey: row.idempotencyKey, event: row.event },
					normalizedSchemaVersion,
				),
			);
		} catch (error) {
			if (error instanceof MutationConflictError) {
				receipts.push({ receiptId: error.receipt.receiptId, status: "conflict" });
				continue;
			}
			throw error;
		}
	}
	return { receipts };
}

function handleRemember(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
	previousRedaction?: SpoolRedactionMetadata,
): Record<string, unknown> {
	let kind: string;
	try {
		kind = validateMemoryKind(String(body.kind));
	} catch {
		throw new RpcRequestError("kind is unsupported.");
	}
	if (body.project !== undefined && typeof body.project !== "string") {
		throw new RpcRequestError("project must be a string.");
	}
	const intake = applyDaemonIntake(
		{
			kind: body.kind,
			title: body.title,
			body: body.body,
			confidence: body.confidence,
			project: body.project,
		},
		{
			allowlist: ["kind", "title", "body", "confidence", "project"],
			metadataKeys: ["kind", "confidence"],
		},
	);
	const redaction = mergedRedaction(intake, previousRedaction);
	const trustedSensitivity: SensitivityV1 =
		redaction.redaction_degraded || redaction.sensitivity === "secret"
			? "secret"
			: redaction.sensitivity === "private"
				? "private"
				: redaction.local_only
					? "local_only"
					: "eligible";
	const payload: Record<string, unknown> = { ...intake.payload, kind };
	if (redaction.sensitivity === "secret" || redaction.redaction_degraded) {
		delete payload.title;
		delete payload.body;
		delete payload.project;
	}
	// Repository identity is derived server-side from the caller's cwd, exactly
	// like the raw-event path above — the caller supplies a probe input, never
	// the identity itself. Without it, a restricted memory would default to
	// secret under the store's pair invariant.
	const rememberRepositoryIdentity =
		typeof body.cwd === "string" && body.cwd.trim() ? resolveRepositoryIdentity(body.cwd) : null;
	const confidence = payload.confidence ?? 0.5;
	if (
		typeof confidence !== "number" ||
		!Number.isFinite(confidence) ||
		confidence < 0 ||
		confidence > 1
	) {
		throw new RpcRequestError("confidence must be a number between 0 and 1.");
	}
	const receipt = dispatchClassA(ctx.writer, {
		method: "POST /v1/memories/record",
		idempotencyKey: String(body.idempotencyKey),
		// The derived repository identity is part of the mutation's identity:
		// the same idempotencyKey replayed from a different repository must
		// conflict, not replay the first repository's receipt.
		payload: { ...payload, redaction, repositoryIdentity: rememberRepositoryIdentity },
		apply: () => {
			const sessionId = ensureSession(ctx.writer, optionalString(payload.project));
			const memoryId = ctx.store.rememberTrusted(
				sessionId,
				kind,
				String(payload.title ?? ""),
				String(payload.body ?? ""),
				confidence,
				undefined,
				redaction,
				{ sensitivity: trustedSensitivity, repositoryIdentity: rememberRepositoryIdentity },
			);
			return { memoryId };
		},
	});
	return {
		receiptId: receipt.receiptId,
		memoryId: receipt.result.memoryId,
	};
}

export function dispatchSpoolMutation(
	ctx: DaemonRpcContext,
	entry: SpoolEntry,
): SpoolImportOutcome {
	if (isMaintenanceMode(ctx)) throw new Error("maintenance mode is active");
	try {
		if (entry.method === "POST /v1/events") {
			const persisted = persistEventWithReceipt(
				ctx,
				entry.body,
				NORMALIZED_SCHEMA_VERSION,
				entry.redaction,
			);
			if (
				persisted.result.status === "conflict" ||
				persisted.receiptPayloadHash !== persisted.incomingPayloadHash
			) {
				return "conflict";
			}
		} else {
			handleRemember(ctx, entry.body, entry.redaction);
		}
		return "committed";
	} catch (error) {
		if (error instanceof MutationConflictError) return "conflict";
		throw error;
	}
}

function handleForget(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
): Record<string, unknown> {
	const id = requirePositiveInt(body.id);
	const expectedRevision =
		body.expectedRevision === undefined ? undefined : requirePositiveInt(body.expectedRevision);
	const receipt = dispatchClassA(ctx.writer, {
		method: "DELETE /v1/memories/:id",
		idempotencyKey: String(body.requestId),
		payload: { id, expectedRevision },
		apply: () => {
			const current = ctx.store.get(id);
			if (!current) {
				throw new RpcRequestError(`Memory ${id} not found.`, "not_found");
			}
			if (expectedRevision !== undefined) {
				if (Number(current.rev ?? 0) !== expectedRevision) {
					throw new Error("revision mismatch.");
				}
			}
			ctx.store.forget(id);
			return { forgotten: id };
		},
	});
	return { receiptId: receipt.receiptId, status: receipt.status };
}

const MCP_SEARCH_SURFACES: Record<string, RetrievalSurface> = {
	search: "mcp_search",
	search_index: "mcp_search_index",
	recent: "mcp_recent",
	timeline: "mcp_timeline",
	get_many: "mcp_get_observations",
	explain: "mcp_explain",
	expand: "mcp_expand",
};

function mcpRetrievalSurface(
	method: RpcMethod,
	body: Record<string, unknown>,
	nativeCliVersion: string,
): RetrievalSurface | null {
	if (nativeCliVersion !== "mcp-stdio") return null;
	if (method === "POST /v1/context/pack") return "mcp_pack";
	if (method === "GET /v1/memories/:id") return "mcp_get";
	return method === "POST /v1/search" ? (MCP_SEARCH_SURFACES[String(body.mode)] ?? null) : null;
}

function returnedMemoryIds(value: unknown): number[] {
	if (!Array.isArray(value)) return [];
	const ids: number[] = [];
	for (const item of value) {
		const id =
			typeof item === "number"
				? item
				: item && typeof item === "object" && !Array.isArray(item)
					? (item as Record<string, unknown>).id
					: null;
		if (typeof id === "number" && Number.isInteger(id) && id > 0 && !ids.includes(id)) {
			ids.push(id);
		}
	}
	return ids;
}

function mcpRetrievalResult(
	surface: RetrievalSurface,
	result: Record<string, unknown>,
): { failed: boolean; memoryIds: number[] } {
	if (surface === "mcp_get") {
		return { failed: false, memoryIds: returnedMemoryIds(result.item ? [result.item] : []) };
	}
	if (surface === "mcp_pack") {
		const pack = result.pack;
		return {
			failed: false,
			memoryIds: returnedMemoryIds(
				pack && typeof pack === "object" && !Array.isArray(pack)
					? (pack as Record<string, unknown>).item_ids
					: [],
			),
		};
	}
	if (surface === "mcp_explain") {
		const explanation = result.items;
		const payload =
			explanation && typeof explanation === "object" && !Array.isArray(explanation)
				? (explanation as Record<string, unknown>)
				: {};
		const failed = Array.isArray(payload.errors)
			? payload.errors.some(
					(error) =>
						error != null &&
						typeof error === "object" &&
						(error as Record<string, unknown>).code === "INVALID_ARGUMENT" &&
						(error as Record<string, unknown>).field === "query",
				)
			: false;
		return { failed, memoryIds: failed ? [] : returnedMemoryIds(payload.items) };
	}
	return { failed: false, memoryIds: returnedMemoryIds(result.items) };
}

function mcpAttemptId(surface: RetrievalSurface, requestId: string): string {
	const hex = createHash("sha256")
		.update(`${surface}\0${requestId}`, "utf8")
		.digest("hex")
		.slice(0, 32);
	return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-8${hex.slice(13, 16)}-${(
		(Number.parseInt(hex[16] ?? "0", 16) & 0x3) | 0x8
	).toString(16)}${hex.slice(17, 20)}-${hex.slice(20)}`;
}

function recordMcpRpcRetrieval(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
	surface: RetrievalSurface,
	startedAt: Date,
	completedAt: Date,
	failed: boolean,
	memoryIds: number[],
): boolean {
	const requestId = String(body.requestId);
	const retrievalStatus: RetrievalStatus = failed
		? "failed"
		: memoryIds.length > 0
			? "succeeded"
			: "no_results";
	const input = {
		attemptId: mcpAttemptId(surface, requestId),
		surface,
		trigger: "explicit" as const,
		startedAt: startedAt.toISOString(),
		completedAt: completedAt.toISOString(),
		retrievalStatus,
		deliveryStatus: "not_attempted" as const,
		candidateIds: failed ? [] : memoryIds,
		selectedIds: failed ? [] : memoryIds,
		recorderVersion: "mcp-retrieval-v1",
		source: "mcp",
		requestId,
		latencyMs: Math.max(0, completedAt.getTime() - startedAt.getTime()),
		failureCode: failed ? "tool_failed" : undefined,
		failureStage: failed ? "retrieval" : undefined,
	};
	const outcome = recordRetrievalSurface(ctx.store.db, input);
	if (outcome.ok) return true;
	if (
		!failed &&
		outcome.reason === "idempotency_conflict" &&
		(retrievalStatus === "succeeded" || retrievalStatus === "no_results")
	) {
		return reconcileFailedRetrievalSurface(ctx.store.db, input).ok;
	}
	return false;
}

function safeRecordMcpRpcRetrieval(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
	surface: RetrievalSurface,
	startedAt: Date,
	failed: boolean,
	result?: Record<string, unknown>,
): boolean {
	try {
		const retrieval = failed
			? { failed: true, memoryIds: [] }
			: mcpRetrievalResult(surface, result ?? {});
		const recorded = recordMcpRpcRetrieval(
			ctx,
			body,
			surface,
			startedAt,
			new Date(),
			retrieval.failed,
			retrieval.memoryIds,
		);
		return recorded && !retrieval.failed && retrieval.memoryIds.length > 0;
	} catch {
		// MCP results must remain independent from local retrieval diagnostics.
		return false;
	}
}

function handleRetrievalRpc(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
	surface: RetrievalSurface | null,
	retrieve: () => Record<string, unknown>,
): Record<string, unknown> {
	if (!surface) return retrieve();
	const startedAt = new Date();
	let result: Record<string, unknown>;
	try {
		result = retrieve();
	} catch (error) {
		safeRecordMcpRpcRetrieval(ctx, body, surface, startedAt, true);
		throw error;
	}
	const hasResults = safeRecordMcpRpcRetrieval(ctx, body, surface, startedAt, false, result);
	return hasResults
		? { ...result, retrievalAttemptId: mcpAttemptId(surface, String(body.requestId)) }
		: result;
}

function handlePack(ctx: DaemonRpcContext, body: Record<string, unknown>): Record<string, unknown> {
	const destinationBoundary = daemonReadBoundary(ctx, "daemon_pack");
	const filters = parseMemoryFilters(body.filters);
	const limit = parseBoundedInteger(body.limit, 8, 1, 50, "limit");
	const tokenBudget =
		body.tokenBudget === undefined
			? null
			: parseBoundedInteger(body.tokenBudget, 0, 0, Number.MAX_SAFE_INTEGER, "tokenBudget");
	if (body.trace !== undefined && typeof body.trace !== "boolean") {
		throw new RpcRequestError("trace must be a boolean.");
	}
	const receipt = dispatchClassA(ctx.writer, {
		method: "POST /v1/context/pack",
		idempotencyKey: String(body.requestId),
		payload: {
			context: body.context,
			limit: body.limit,
			tokenBudget: body.tokenBudget,
			filters: body.filters,
			trace: body.trace,
		},
		apply: () => ({ recorded: true }),
	});
	const context = String(body.context);
	if (body.trace === true) {
		const artifacts = ctx.store.buildMemoryPackWithTrace(
			context,
			limit,
			tokenBudget,
			filters,
			undefined,
			destinationBoundary,
		);
		return {
			pack: artifacts.response,
			trace: artifacts.trace,
			retrievalReceiptId: receipt.receiptId,
		};
	}
	const pack = ctx.store.buildMemoryPack(context, limit, tokenBudget, filters, destinationBoundary);
	return {
		pack,
		retrievalReceiptId: receipt.receiptId,
	};
}

const FILE_CONTEXT_ATTEMPT_ID =
	/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const FILE_CONTEXT_RETRIEVAL_STATUSES = new Set(["succeeded", "no_results", "skipped", "failed"]);

function repositoryRelativePath(value: unknown): string | null {
	if (typeof value !== "string") return null;
	const path = value.trim().replaceAll("\\", "/");
	if (
		!path ||
		!isSafePersistedText(path, 4096) ||
		path.startsWith("/") ||
		/^[A-Za-z]:\//.test(path) ||
		path.split("/").includes("..")
	) {
		return null;
	}
	return path;
}

function fileContextText(value: unknown, field: string, maxBytes: number): string | null {
	if (value === undefined || value === null || value === "") return null;
	if (typeof value !== "string" || !isSafePersistedText(value, maxBytes)) {
		throw new RpcRequestError(`${field} is invalid.`);
	}
	return value;
}

function fileContextTimestamp(value: unknown, field: string): string {
	const timestamp = fileContextText(value, field, 64);
	if (!timestamp || !Number.isFinite(Date.parse(timestamp))) {
		throw new RpcRequestError(`${field} must be an ISO timestamp.`);
	}
	return timestamp;
}

function handleFileContextRetrieval(
	ctx: DaemonRpcContext,
	body: FileContextRetrievalAttempt,
): Record<string, unknown> {
	if (!FILE_CONTEXT_ATTEMPT_ID.test(String(body.attemptId))) {
		throw new RpcRequestError("attemptId is invalid.");
	}
	if (!FILE_CONTEXT_RETRIEVAL_STATUSES.has(String(body.retrievalStatus))) {
		throw new RpcRequestError("retrievalStatus is invalid.");
	}
	const startedAt = fileContextTimestamp(body.startedAt, "startedAt");
	const completedAt = fileContextTimestamp(body.completedAt, "completedAt");
	const startedMs = Date.parse(startedAt);
	const completedMs = Date.parse(completedAt);
	if (completedMs < startedMs) throw new RpcRequestError("completedAt precedes startedAt.");
	const candidateIds = parseIds(body.candidateIds);
	const selectedIds = parseIds(body.selectedIds);
	const candidateCount = parseBoundedInteger(
		body.candidateCount,
		candidateIds.length,
		0,
		200,
		"candidateCount",
	);
	const repositoryPath =
		body.repositoryPath === undefined ? null : repositoryRelativePath(body.repositoryPath);
	if (body.repositoryPath !== undefined && !repositoryPath) {
		throw new RpcRequestError("repositoryPath must be a safe repository-relative path.");
	}
	const project = fileContextText(body.project, "project", 512);
	const sourceSessionId = fileContextText(body.sourceSessionId, "sourceSessionId", 512);
	const failureCode = fileContextText(body.failureCode, "failureCode", 128);
	const failureStage = fileContextText(body.failureStage, "failureStage", 128);
	const outcome = recordRetrievalSurface(ctx.store.db, {
		attemptId: body.attemptId,
		surface: "file_context",
		trigger: "automatic",
		startedAt,
		completedAt,
		retrievalStatus: body.retrievalStatus,
		deliveryStatus: "not_attempted",
		candidateIds,
		candidateCount,
		selectedIds,
		recorderVersion: "claude-file-context-v1",
		sessionId: resolveRetrievalSession(ctx.store.db, "claude", sourceSessionId),
		source: "claude",
		streamId: sourceSessionId,
		sourceSessionId,
		latencyMs: completedMs - startedMs,
		project,
		mode: "claude_pre_tool_use_read",
		filters: project ? { project } : undefined,
		repositoryPaths: repositoryPath ? [repositoryPath] : undefined,
		failureCode,
		failureStage,
	});
	if (!outcome.ok) {
		if (outcome.reason === "invalid_input" || outcome.reason === "idempotency_conflict") {
			throw new RpcRequestError("file-context retrieval attempt is invalid.");
		}
		return { recorded: false, errorCode: outcome.errorCode };
	}
	return { recorded: true, inserted: outcome.value.inserted };
}

function handleFileContextDelivery(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
): Record<string, unknown> {
	const attemptId = String(body.attemptId);
	if (!FILE_CONTEXT_ATTEMPT_ID.test(attemptId)) {
		throw new RpcRequestError("attemptId is invalid.");
	}
	const status = String(body.status);
	if (status !== "handed_off" && status !== "failed") {
		throw new RpcRequestError("status is invalid.");
	}
	const outcome = tryUpdateRetrievalDelivery(ctx.store.db, attemptId, status);
	if (!outcome.ok) {
		if (outcome.reason !== "storage_unavailable") {
			throw new RpcRequestError("file-context retrieval delivery is invalid.");
		}
		return { updated: false, errorCode: outcome.errorCode };
	}
	return { updated: outcome.value.changed };
}

function handleSearch(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
): Record<string, unknown> {
	const destinationBoundary = daemonReadBoundary(ctx, "daemon_search");
	const mode = String(body.mode);
	const filters = parseMemoryFilters(body.filters);
	const limit = parseBoundedInteger(body.limit, 10, 1, 100, "limit");
	const ids = parseIds(body.ids);
	const query = typeof body.query === "string" ? body.query : null;
	const repositoryPath = repositoryRelativePath(body.repositoryPath);
	const memoryId = body.memoryId === undefined ? null : requirePositiveInt(body.memoryId);
	const depthBefore = parseBoundedInteger(body.depthBefore, 3, 0, 100, "depthBefore");
	const depthAfter = parseBoundedInteger(body.depthAfter, 3, 0, 100, "depthAfter");
	const supportedMode = [
		"search",
		"search_index",
		"find_by_file",
		"recent",
		"timeline",
		"get_many",
		"explain",
		"expand",
	].includes(mode);
	if (!supportedMode) throw new RpcRequestError(`Unsupported search mode: ${mode}`);
	if ((mode === "search" || mode === "search_index") && !query) {
		throw new RpcRequestError("query is required for search mode.");
	}
	if (mode === "find_by_file" && !repositoryPath) {
		throw new RpcRequestError("repositoryPath must be a safe repository-relative path.");
	}
	if ((mode === "get_many" || mode === "expand") && ids.length === 0) {
		throw new RpcRequestError(`ids are required for ${mode} mode.`);
	}
	if (body.includePackContext !== undefined && typeof body.includePackContext !== "boolean") {
		throw new RpcRequestError("includePackContext must be a boolean.");
	}
	const receipt = dispatchClassA(ctx.writer, {
		method: "POST /v1/search",
		idempotencyKey: String(body.requestId),
		payload: {
			mode: body.mode,
			query: body.query,
			repositoryPath,
			ids: body.ids,
			memoryId,
			depthBefore,
			depthAfter,
			includePackContext: body.includePackContext,
			filters: body.filters,
			limit: body.limit,
		},
		apply: () => ({ recorded: true }),
	});
	let items: unknown;
	if (mode === "search" || mode === "search_index") {
		items = ctx.store.search(query as string, limit, filters, destinationBoundary);
	} else if (mode === "find_by_file") {
		items = ctx.store.findByFile(
			repositoryPath as string,
			{
				limit,
				...(filters?.project ? { project: filters.project } : {}),
			},
			destinationBoundary,
		);
	} else if (mode === "recent") {
		items = ctx.store.recent(limit, filters, 0, destinationBoundary);
	} else if (mode === "timeline") {
		items = ctx.store.timeline(
			query,
			memoryId,
			depthBefore,
			depthAfter,
			filters,
			destinationBoundary,
		);
	} else if (mode === "get_many") {
		items = ids.flatMap((id) =>
			ctx.store
				.timeline(null, id, 0, 0, filters, destinationBoundary)
				.filter((item) => item.id === id),
		);
	} else if (mode === "explain") {
		items = ctx.store.explain(
			query,
			ids,
			limit,
			filters,
			{ includePackContext: body.includePackContext === true },
			destinationBoundary,
		);
	} else if (mode === "expand") {
		const seen = new Set<number>();
		items = ids.flatMap((id) => {
			return ctx.store
				.timeline(null, id, depthBefore, depthAfter, filters, destinationBoundary)
				.filter((item) => {
					if (seen.has(item.id)) return false;
					seen.add(item.id);
					return true;
				});
		});
	}
	return {
		items,
		retrievalReceiptId: receipt.receiptId,
	};
}

function handleMemoryGet(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
): Record<string, unknown> {
	const id = requirePositiveInt(body.id);
	const filters = parseMemoryFilters({
		...(body.project === undefined ? {} : { project: body.project }),
		...(body.kind === undefined ? {} : { kind: body.kind }),
	});
	const receipt = dispatchClassA(ctx.writer, {
		method: "GET /v1/memories/:id",
		idempotencyKey: String(body.requestId),
		payload: { id, project: body.project, kind: body.kind },
		apply: () => ({ recorded: true }),
	});
	let item = ctx.store.get(id, daemonReadBoundary(ctx, "daemon_get"));
	if (
		item &&
		((filters?.project !== undefined && item.project !== filters.project) ||
			(filters?.kind !== undefined && item.kind !== filters.kind))
	) {
		item = null;
	}
	return {
		item,
		retrievalReceiptId: receipt.receiptId,
	};
}

async function handleView(
	ctx: DaemonRpcContext,
	body: Record<string, unknown>,
): Promise<Record<string, unknown>> {
	const collection = String(body.collection);
	if (!VIEW_COLLECTIONS.has(collection)) {
		throw new RpcRequestError(`Unknown view collection: ${collection}`);
	}
	if (body.limit !== undefined) parseBoundedInteger(body.limit, 20, 1, 1_000, "limit");
	if (body.offset !== undefined) {
		parseBoundedInteger(body.offset, 0, 0, 1_000_000, "offset");
	}
	if (body.sessionId !== undefined) requirePositiveInt(body.sessionId);
	for (const field of ["project", "kind"] as const) {
		if (
			body[field] !== undefined &&
			(typeof body[field] !== "string" || Buffer.byteLength(body[field], "utf8") > 4_096)
		) {
			throw new RpcRequestError(`${field} must be a bounded string.`);
		}
	}
	if (body.scope !== undefined && body.scope !== "mine" && body.scope !== "theirs") {
		throw new RpcRequestError("scope must be mine or theirs.");
	}
	return (await ctx.viewerRead(body)) as Record<string, unknown>;
}

function ensureSession(db: WriterActor, project: string | null): number {
	const existing =
		project === null
			? (db
					.prepare("SELECT id FROM sessions WHERE project IS NULL ORDER BY id DESC LIMIT 1")
					.get() as { id: number } | undefined)
			: (db
					.prepare("SELECT id FROM sessions WHERE project = ? ORDER BY id DESC LIMIT 1")
					.get(project) as { id: number } | undefined);
	if (existing) return existing.id;
	const result = db
		.prepare("INSERT INTO sessions(started_at, project) VALUES (?, ?)")
		.run(new Date().toISOString(), project);
	return Number(result.lastInsertRowid);
}

function asObject(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new RpcRequestError("Expected an object.");
	}
	return value as Record<string, unknown>;
}

function parseMemoryFilters(value: unknown): MemoryFilters | undefined {
	if (value === undefined) return undefined;
	const filters = asObject(value);
	const extra = unknownFields(filters, FILTER_FIELDS);
	if (extra.length > 0) throw new RpcRequestError(`Unknown filter field: ${extra[0]}`);
	for (const [field, item] of Object.entries(filters)) {
		if (STRING_FILTER_FIELDS.has(field) && typeof item !== "string") {
			throw new RpcRequestError(`filters.${field} must be a string.`);
		}
		if (
			STRING_ARRAY_FILTER_FIELDS.has(field) &&
			(!Array.isArray(item) || !item.every((entry) => typeof entry === "string"))
		) {
			throw new RpcRequestError(`filters.${field} must be an array of strings.`);
		}
		if (
			STRING_OR_ARRAY_FILTER_FIELDS.has(field) &&
			typeof item !== "string" &&
			(!Array.isArray(item) || !item.every((entry) => typeof entry === "string"))
		) {
			throw new RpcRequestError(`filters.${field} must be a string or array of strings.`);
		}
		if (
			BOOLEAN_OR_STRING_FILTER_FIELDS.has(field) &&
			typeof item !== "boolean" &&
			typeof item !== "string"
		) {
			throw new RpcRequestError(`filters.${field} must be a boolean or string.`);
		}
		if (NUMBER_FILTER_FIELDS.has(field) && (typeof item !== "number" || !Number.isFinite(item))) {
			throw new RpcRequestError(`filters.${field} must be a finite number.`);
		}
		if (
			field === "session_id" &&
			(typeof item !== "number" || !Number.isInteger(item) || item <= 0)
		) {
			throw new RpcRequestError("filters.session_id must be a positive integer.");
		}
	}
	return filters as MemoryFilters;
}

function parseBoundedInteger(
	value: unknown,
	fallback: number,
	minimum: number,
	maximum: number,
	field: string,
): number {
	if (value === undefined) return fallback;
	if (typeof value !== "number" || !Number.isInteger(value) || value < minimum || value > maximum) {
		throw new RpcRequestError(`${field} must be an integer between ${minimum} and ${maximum}.`);
	}
	return value;
}

function parseIds(value: unknown): number[] {
	if (value === undefined) return [];
	if (!Array.isArray(value) || value.length > 200) {
		throw new RpcRequestError("ids must be an array with at most 200 entries.");
	}
	return value.map((entry) => requirePositiveInt(entry));
}

function optionalString(value: unknown): string | null {
	return typeof value === "string" && value.length > 0 ? value : null;
}

function requirePositiveInt(value: unknown): number {
	if (typeof value === "number" && Number.isInteger(value) && value > 0) return value;
	if (typeof value === "string" && /^[1-9][0-9]*$/.test(value)) return Number(value); // rpc-v1 R28
	throw new RpcRequestError("id is invalid.");
}

export async function dispatchDaemonRpc(
	raw: string,
	ctx: DaemonRpcContext,
): Promise<RpcSuccess | TypedRpcError> {
	const started = (ctx.now ?? Date.now)();
	const request = parseRequest(raw);
	if ("error" in request) return request;
	const handshake = handshakeError(request);
	if (handshake) return handshake;
	if (!isRpcMethod(request.method)) {
		return typedError("unknown_method", `Unknown RPC method: ${request.method}`);
	}
	const deadlineMs = ctx.deadlineMs ?? rpcDeadlineForMethod(request.method);
	const body = request.body ?? {};
	const extra = unknownFields(body, METHOD_BODY_FIELDS[request.method]);
	if (extra.length > 0) {
		return typedError("unknown_field", `Unknown field for ${request.method}: ${extra[0]}`);
	}
	const legacyUnknownDoctorAttempt =
		request.method === "POST /v1/processing-jobs/:id/doctor-retry" &&
		body.expectedProviderFingerprint === null &&
		body.expectedManifestFingerprint === null;
	for (const field of METHOD_REQUIRED_FIELDS[request.method]) {
		const value = body[field];
		const nullableLegacyFingerprint =
			legacyUnknownDoctorAttempt &&
			(field === "expectedProviderFingerprint" || field === "expectedManifestFingerprint");
		if (value === undefined || (value === null && !nullableLegacyFingerprint)) {
			return typedError("invalid_request", `${field} is required.`);
		}
		if (
			(field === "idempotencyKey" ||
				field === "requestId" ||
				field === "operationId" ||
				field === "payloadHash" ||
				field === "reason" ||
				field === "backupId" ||
				field === "kind" ||
				field === "title" ||
				field === "body" ||
				field === "collection" ||
				field === "mode" ||
				field === "context") &&
			typeof value !== "string"
		) {
			return typedError("invalid_request", `${field} is required.`);
		}
		if (typeof value === "string" && value.length === 0) {
			return typedError("invalid_request", `${field} is required.`);
		}
		if (PERSISTED_ID_FIELDS.has(field) && !isSafePersistedText(value, 256)) {
			return typedError("invalid_request", `${field} is invalid.`);
		}
		if (field === "reason" && !isSafePersistedText(value, 1024)) {
			return typedError("invalid_request", "reason is invalid.");
		}
	}
	const elapsed = (ctx.now ?? Date.now)() - started;
	if (elapsed >= deadlineMs) {
		return typedError("deadline_exceeded", "RPC hard deadline exceeded.", true);
	}
	if (isMaintenanceMode(ctx) && MAINTENANCE_BLOCKED_METHODS.has(request.method)) {
		return typedError(
			"maintenance_mode",
			"The daemon is in maintenance mode; retryable mutations must be queued locally.",
			true,
		);
	}
	try {
		return {
			id: request.id,
			result: await handleMethod(
				request.method,
				body,
				ctx,
				request.normalized_schema_version,
				request.native_cli_version,
			),
		};
	} catch (error) {
		if (error instanceof RpcRequestError) {
			return typedError(error.code, error.message, error.retryable);
		}
		if (error instanceof BackupRequestError) {
			return typedError(error.code, error.message);
		}
		if (error instanceof DaemonOperationRequestError) {
			return typedError(error.code, error.message);
		}
		if (error instanceof MutationConflictError) {
			return typedError(error.code, error.message);
		}
		if (error instanceof ProcessingResumeError) {
			return typedError(error.code, error.message, error.retryable);
		}
		return typedError("internal_error", "RPC handler failed.");
	}
}

export function attachDaemonRpc(connection: Socket, ctx: DaemonRpcContext): void {
	let buffer = Buffer.alloc(0);
	let done = false;
	let stopAfterResponse = false;
	let stopRequested = false;
	const requestStopAfterResponse = () => {
		if (!stopAfterResponse || stopRequested) return;
		stopRequested = true;
		ctx.onStop();
	};
	connection.on("error", () => {
		connection.destroy();
		requestStopAfterResponse();
	});
	const finish = (payload?: RpcSuccess | TypedRpcError) => {
		if (payload) {
			stopAfterResponse ||=
				ctx.restoreState?.active === true &&
				"result" in payload &&
				payload.result.restartRequired === true;
		}
		if (done) {
			requestStopAfterResponse();
			return;
		}
		done = true;
		if (payload) {
			if (connection.destroyed) {
				requestStopAfterResponse();
				return;
			}
			try {
				connection.end(`${JSON.stringify(payload)}\n`, requestStopAfterResponse);
				return;
			} catch {
				// connection already closed
				requestStopAfterResponse();
			}
		}
		connection.destroy();
	};
	let dispatching = false;
	connection.on("data", (chunk: Buffer) => {
		if (done || dispatching) return;
		if (buffer.length + chunk.length > RPC_MAX_BYTES) {
			finish(typedError("payload_too_large", `RPC request exceeds ${RPC_MAX_BYTES} bytes.`));
			return;
		}
		buffer = Buffer.concat([buffer, chunk]);
		const newline = buffer.indexOf(0x0a);
		if (newline < 0) return;
		const line = buffer.subarray(0, newline).toString("utf8");
		buffer = buffer.subarray(newline + 1);
		if (line.startsWith("STOP")) {
			const nonce = line.slice(4).trim();
			if (nonce && nonce === ctx.identity.nonce) {
				connection.end(`${JSON.stringify({ status: "stopping" })}\n`);
				ctx.onStop();
			} else {
				connection.end(`${JSON.stringify({ status: "mismatch" })}\n`);
			}
			done = true;
			return;
		}
		dispatching = true;
		try {
			const request = JSON.parse(line) as { method?: unknown };
			if (typeof request.method === "string") {
				connection.setTimeout(ctx.deadlineMs ?? rpcDeadlineForMethod(request.method));
			}
		} catch {
			// Canonical parsing below returns the typed invalid_json response.
		}
		void dispatchDaemonRpc(line, ctx).then(finish, () => {
			finish(typedError("internal_error", "RPC handler failed."));
		});
	});
	connection.setTimeout(ctx.deadlineMs ?? RPC_DEFAULT_DEADLINE_MS, () => {
		finish(typedError("deadline_exceeded", "RPC hard deadline exceeded.", true));
	});
}
