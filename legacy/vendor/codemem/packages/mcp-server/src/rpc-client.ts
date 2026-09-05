import { createHash, randomUUID } from "node:crypto";
import { closeSync, constants, fstatSync, openSync, readSync } from "node:fs";
import { join } from "node:path";
import {
	type AgentMemoryConfig,
	backupPayloadHash,
	callDaemonRpc,
	hashMutationPayload,
	LOCAL_API_VERSION,
	NORMALIZED_EVENT_FIELDS,
	NORMALIZED_SCHEMA_VERSION,
	parseAgentMemoryToml,
	preprocessAdapterEvent,
	RPC_CAPABILITY_HASH,
	type RpcMethod,
	resolveProjectRoot,
	resolveRuntimeDataDir,
	resolveStorageLayout,
	rpcDeadlineForMethod,
	type SpoolRedactionMetadata,
	sealDegradedNormalizedEvent,
	spoolMutation,
	VERSION,
	validateNormalizedEvent,
} from "@codemem/core";

const PROJECT_CONFIG_MAX_BYTES = 64 * 1024;
const PROCESS_REQUEST_SCOPE = randomUUID();

const RPC_FIELDS = {
	"GET /v1/health": [],
	"GET /v1/doctor": [],
	"GET /v1/view": ["collection", "sessionId", "project", "kind", "scope", "limit", "offset"],
	"GET /v1/memories/:id": ["id", "requestId", "project", "kind"],
	"DELETE /v1/memories/:id": ["id", "requestId", "expectedRevision"],
	"GET /v1/backup/list": [],
	"POST /v1/backup/create": ["operationId", "payloadHash", "reason"],
	"POST /v1/backup/verify": ["backupId"],
	"POST /v1/backup/restore": ["operationId", "payloadHash", "backupId"],
	"POST /v1/events": ["idempotencyKey", "event"],
	"POST /v1/context/pack": ["requestId", "context", "limit", "tokenBudget", "filters", "trace"],
	"POST /v1/search": [
		"requestId",
		"mode",
		"query",
		"ids",
		"memoryId",
		"depthBefore",
		"depthAfter",
		"includePackContext",
		"filters",
		"limit",
	],
	"POST /v1/retrieval/file-context/delivery": ["attemptId", "status"],
	"POST /v1/memories/record": [
		"idempotencyKey",
		"kind",
		"title",
		"body",
		"confidence",
		"project",
		"cwd",
	],
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
	"POST /v1/operations/export": ["operationId", "payloadHash", "outputPath", "filters"],
	"POST /v1/operations/import": [
		"operationId",
		"payloadHash",
		"inputPath",
		"remapProject",
		"dryRun",
	],
	"GET /v1/operations/:id": ["id"],
} as const satisfies Partial<Record<RpcMethod, readonly string[]>>;

const METADATA_FIELDS = new Set([
	"id",
	"attemptId",
	"requestId",
	"idempotencyKey",
	"mode",
	"ids",
	"memoryId",
	"depthBefore",
	"depthAfter",
	"includePackContext",
	"limit",
	"tokenBudget",
	"trace",
	"collection",
	"sessionId",
	"offset",
	"scope",
	"expectedRevision",
	"kind",
	"confidence",
	"dryRun",
	"state",
	"status",
	"submittedAfter",
	"operationId",
	"payloadHash",
	"backupId",
	"reason",
	"outputPath",
	"inputPath",
	"remapProject",
	"filters",
]);

export type McpRpcError = { code: string; message: string; retryable: boolean };
export type McpRpcOutcome =
	| {
			ok: true;
			result: Record<string, unknown>;
			finalizeDelivery?: (status: "handed_off" | "failed") => Promise<void>;
	  }
	| { ok: false; error: McpRpcError };

export interface McpRpcClient {
	request(method: SupportedMcpRpcMethod, body: Record<string, unknown>): Promise<McpRpcOutcome>;
	requestWithSpool(
		method: SpoolableMcpRpcMethod,
		body: Record<string, unknown>,
	): Promise<McpRpcOutcome>;
	remember(body: Record<string, unknown>): Promise<McpRpcOutcome>;
}

export type SupportedMcpRpcMethod = keyof typeof RPC_FIELDS;
export type SpoolableMcpRpcMethod = "POST /v1/events" | "POST /v1/memories/record";

export interface McpRpcClientOptions {
	dataDir?: string;
	dbPath?: string;
	cwd?: () => string;
}

type PreparedRequest = {
	body: Record<string, unknown>;
	config?: AgentMemoryConfig;
	redaction: SpoolRedactionMetadata;
};

export function mcpRequestId(
	toolName: string,
	requestId: string | number | undefined,
	requestScope: string = PROCESS_REQUEST_SCOPE,
): string {
	const value = requestId === undefined ? randomUUID() : String(requestId);
	return createHash("sha256")
		.update(`${requestScope}\0${toolName}\0${value}`, "utf8")
		.digest("hex");
}

export function createMcpRpcClient(options: McpRpcClientOptions = {}): McpRpcClient {
	const dataDir = () => resolveRuntimeDataDir(options);
	const cwd = options.cwd ?? (() => process.cwd());

	const prepare = (
		method: SupportedMcpRpcMethod,
		body: Record<string, unknown>,
	): PreparedRequest => {
		const fields = RPC_FIELDS[method];
		if (
			method === "POST /v1/operations/export" ||
			method === "POST /v1/operations/import" ||
			method === "POST /v1/backup/create" ||
			method === "POST /v1/backup/verify" ||
			method === "POST /v1/backup/restore" ||
			method === "GET /v1/processing-jobs/:id" ||
			method === "POST /v1/processing-jobs/:id/doctor-retry"
		) {
			// Keep operation, backup, and doctor control metadata intact.
			const preparedBody = Object.fromEntries(
				fields.filter((field) => Object.hasOwn(body, field)).map((field) => [field, body[field]]),
			);
			if (method === "POST /v1/backup/create") {
				const config = loadProjectConfig(cwd());
				const redacted = preprocessAdapterEvent(
					{ reason: preparedBody.reason },
					{ allowlist: ["reason"], metadataKeys: [], config },
				);
				const reason = redacted.degraded
					? "[REDACTED:degraded]"
					: String(redacted.payload.reason ?? "");
				preparedBody.reason = reason;
				preparedBody.payloadHash = backupPayloadHash(reason);
				return {
					body: preparedBody,
					config,
					redaction: {
						sensitivity: redacted.sensitivity,
						secret_rules_version: redacted.secret_rules_version,
						redaction_degraded: redacted.degraded,
						private_content_omitted: redacted.private_content_omitted,
						local_only: redacted.local_only,
					},
				};
			}
			if (method === "POST /v1/operations/export" || method === "POST /v1/operations/import") {
				const { operationId: _operationId, payloadHash: _payloadHash, ...request } = preparedBody;
				preparedBody.payloadHash = hashMutationPayload(request);
			}
			return {
				body: preparedBody,
				redaction: {
					sensitivity: "normal",
					secret_rules_version: "",
					redaction_degraded: false,
					private_content_omitted: false,
					local_only: false,
				},
			};
		}
		const config = fields.length === 0 ? undefined : loadProjectConfig(cwd());
		if (method === "POST /v1/events") {
			const rawEvent = body.event;
			if (!rawEvent || typeof rawEvent !== "object" || Array.isArray(rawEvent)) {
				throw new Error("event must be an object");
			}
			const redacted = preprocessAdapterEvent(rawEvent, {
				allowlist: [...NORMALIZED_EVENT_FIELDS],
				metadataKeys: NORMALIZED_EVENT_FIELDS.filter((field) => field !== "payload"),
				config,
			});
			const event = redacted.degraded
				? sealDegradedNormalizedEvent(redacted.payload)
				: redacted.payload;
			if (!Object.hasOwn(event, "payload")) event.payload = {};
			const sensitivity = redacted.degraded ? "secret" : redacted.sensitivity;
			event.sensitivity = sensitivity;
			if (sensitivity === "secret") event.payload = {};
			validateNormalizedEvent(event);
			if (event.idempotencyKey !== body.idempotencyKey) {
				throw new Error("event.idempotencyKey must match the request envelope");
			}
			return {
				body: { idempotencyKey: body.idempotencyKey, event },
				config,
				redaction: {
					sensitivity,
					secret_rules_version: redacted.secret_rules_version,
					redaction_degraded: redacted.degraded,
					private_content_omitted: redacted.private_content_omitted,
					local_only: redacted.local_only,
				},
			};
		}
		const redacted = preprocessAdapterEvent(body, {
			allowlist: [...fields],
			metadataKeys: fields.filter((field) => METADATA_FIELDS.has(field)),
			config,
		});
		const preparedBody = redacted.payload;
		if (method === "POST /v1/memories/record" && redacted.degraded) {
			preparedBody.title = "[REDACTED:degraded]";
			preparedBody.body = "[REDACTED:degraded]";
		}
		return {
			body: preparedBody,
			config,
			redaction: {
				sensitivity: redacted.sensitivity,
				secret_rules_version: redacted.secret_rules_version,
				redaction_degraded: redacted.degraded,
				private_content_omitted: redacted.private_content_omitted,
				local_only: redacted.local_only,
			},
		};
	};

	const send = async (
		method: SupportedMcpRpcMethod,
		prepared: PreparedRequest,
	): Promise<McpRpcOutcome> => {
		try {
			const deadlineMs = rpcDeadlineForMethod(method);
			const body =
				method === "POST /v1/events" || method === "POST /v1/memories/record"
					? { ...prepared.body, adapterRedaction: prepared.redaction }
					: prepared.body;
			const response = await callDaemonRpc(
				resolveStorageLayout(dataDir()).socketPath,
				{
					id: randomUUID(),
					method,
					adapter_version: VERSION,
					native_cli_version: "mcp-stdio",
					normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
					local_api_version: LOCAL_API_VERSION,
					capability_hash: RPC_CAPABILITY_HASH,
					body,
				},
				{
					timeoutMs: deadlineMs,
					signal: AbortSignal.timeout(deadlineMs),
				},
			);
			if ("error" in response) {
				return {
					ok: false,
					error:
						response.error.code === "daemon_unavailable"
							? {
									...response.error,
									message: "The local memory daemon is unavailable.",
								}
							: response.error,
				};
			}
			const retrievalAttemptId = response.result.retrievalAttemptId;
			if (typeof retrievalAttemptId !== "string") return { ok: true, result: response.result };
			const result = { ...response.result };
			delete result.retrievalAttemptId;
			return {
				ok: true,
				result,
				finalizeDelivery: async (status) => {
					try {
						await send(
							"POST /v1/retrieval/file-context/delivery",
							prepare("POST /v1/retrieval/file-context/delivery", {
								attemptId: retrievalAttemptId,
								status,
							}),
						);
					} catch {
						// Retrieval diagnostics must never alter the MCP tool result.
					}
				},
			};
		} catch {
			return {
				ok: false,
				error: {
					code: "daemon_unavailable",
					message: "The local memory daemon is unavailable.",
					retryable: true,
				},
			};
		}
	};

	return {
		async request(method, body) {
			try {
				return await send(method, prepare(method, body));
			} catch {
				return {
					ok: false,
					error: {
						code: "policy_unavailable",
						message: "Project memory policy could not be loaded safely.",
						retryable: false,
					},
				};
			}
		},

		async requestWithSpool(method, body) {
			let prepared: PreparedRequest;
			try {
				prepared = prepare(method, body);
			} catch {
				return {
					ok: false,
					error: {
						code: "policy_unavailable",
						message: "Project memory policy could not be loaded safely.",
						retryable: false,
					},
				};
			}
			const response = await send(method, prepared);
			if (response.ok || !response.error.retryable) return response;

			const idempotencyKey = String(prepared.body.idempotencyKey ?? "");
			const queued = spoolMutation(
				{
					method,
					idempotencyKey,
					body: prepared.body,
				},
				{
					dataDir: dataDir(),
					config: prepared.config,
					previousRedaction: prepared.redaction,
				},
			);
			if (queued.status !== "dropped") {
				return { ok: true, result: { status: "queued", duplicate: queued.status === "duplicate" } };
			}
			return {
				ok: false,
				error: {
					code: "spool_write_failed",
					message: "The memory could not be queued safely.",
					retryable: true,
				},
			};
		},

		remember(body) {
			return this.requestWithSpool("POST /v1/memories/record", body);
		},
	};
}

function loadProjectConfig(cwd: string): AgentMemoryConfig | undefined {
	const root = resolveProjectRoot(cwd);
	if (!root) return undefined;
	const path = join(root, ".agent-memory.toml");
	let descriptor: number;
	try {
		descriptor = openSync(path, constants.O_RDONLY | constants.O_NONBLOCK);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
		throw error;
	}
	try {
		const opened = fstatSync(descriptor);
		if (!opened.isFile() || opened.size > PROJECT_CONFIG_MAX_BYTES) {
			throw new Error("Project memory policy is invalid.");
		}
		const buffer = Buffer.alloc(opened.size);
		let length = 0;
		while (length < buffer.length) {
			const bytesRead = readSync(descriptor, buffer, length, buffer.length - length, length);
			if (bytesRead === 0) break;
			length += bytesRead;
		}
		return parseAgentMemoryToml(buffer.subarray(0, length).toString("utf8"));
	} finally {
		closeSync(descriptor);
	}
}
