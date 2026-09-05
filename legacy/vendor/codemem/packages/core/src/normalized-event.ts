import type { ClaudeHookAdapterEvent, ClaudeHookRawEventEnvelope } from "./claude-hooks.js";
import { buildRawEventEnvelopeFromHook } from "./claude-hooks.js";
import type { CodexHookAdapterEvent, CodexHookRawEventEnvelope } from "./codex-hooks.js";
import { buildRawEventEnvelopeFromCodexHook } from "./codex-hooks.js";
import { hashMutationPayload } from "./mutation-dispatcher.js";

export const NORMALIZED_SCHEMA_VERSION = 1;

export const NORMALIZED_EVENT_FIELDS = [
	"schemaVersion",
	"eventId",
	"idempotencyKey",
	"agent",
	"agentInstanceId",
	"parentSessionId",
	"nativeSessionId",
	"nativeTurnId",
	"nativeToolUseId",
	"nativeSequence",
	"projectKey",
	"workspaceKey",
	"branchKey",
	"cwd",
	"gitHeadSha",
	"dirtyTreeFingerprint",
	"kind",
	"occurredAt",
	"model",
	"payload",
	"sourceHash",
	"sensitivity",
	"injectedContextIds",
] as const;

const AGENTS = new Set(["claude-code", "codex", "opencode", "pi", "kimi"]);

const KINDS = new Set([
	"session_started",
	"user_prompted",
	"assistant_completed",
	"tool_started",
	"tool_completed",
	"tool_failed",
	"turn_completed",
	"pre_compact",
	"post_compact",
	"session_idle",
	"session_interrupted",
	"session_ended",
]);

const SENSITIVITIES = new Set(["normal", "private", "secret"]);

const FIELDS = new Set<string>(NORMALIZED_EVENT_FIELDS);
const DEGRADED_COPY_FIELDS = [
	"schemaVersion",
	"eventId",
	"idempotencyKey",
	"agent",
	"kind",
	"occurredAt",
	"sourceHash",
] as const;
const REDACTED_METADATA_VALUE = /^redacted:[a-f0-9]{32}$/;

function redactedMetadataValue(value: unknown): string {
	if (typeof value === "string" && REDACTED_METADATA_VALUE.test(value)) return value;
	return `redacted:${hashMutationPayload(value).slice(0, 32)}`;
}

type HookAdapterEvent = ClaudeHookAdapterEvent | CodexHookAdapterEvent;
type HookEnvelope = ClaudeHookRawEventEnvelope | CodexHookRawEventEnvelope;

function normalizedKind(adapter: HookAdapterEvent): string | null {
	if (adapter.event_type === "session_start") return "session_started";
	if (adapter.event_type === "prompt") return "user_prompted";
	if (adapter.event_type === "assistant") return "assistant_completed";
	if (adapter.event_type === "tool_call") return "tool_started";
	if (adapter.event_type === "tool_result") {
		return adapter.payload.status === "error" ? "tool_failed" : "tool_completed";
	}
	if (adapter.event_type === "session_end") return "session_ended";
	return null;
}

function optionalString(value: unknown): string | undefined {
	return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function buildNormalizedHookEvent(
	agent: "claude-code" | "codex",
	envelope: HookEnvelope | null,
): Record<string, unknown> | null {
	if (!envelope) return null;
	const adapter = envelope.payload._adapter as HookAdapterEvent | undefined;
	if (!adapter || typeof adapter !== "object") return null;
	const kind = normalizedKind(adapter);
	if (!kind) return null;
	const meta = adapter.meta ?? {};
	const cwd = optionalString(envelope.cwd) ?? "unknown";
	const projectKey = optionalString(envelope.project) ?? cwd;
	const nativeToolUseId = optionalString(meta.tool_use_id);
	const nativeTurnId = optionalString(meta.turn_id);
	const sourceHash = hashMutationPayload({
		agent,
		nativeSessionId: envelope.session_stream_id,
		kind,
		nativeToolUseId,
		nativeTurnId,
		payload: adapter.payload,
	});
	return {
		schemaVersion: NORMALIZED_SCHEMA_VERSION,
		eventId: envelope.event_id,
		idempotencyKey: envelope.event_id,
		agent,
		nativeSessionId: envelope.session_stream_id,
		...(nativeTurnId ? { nativeTurnId } : {}),
		...(nativeToolUseId ? { nativeToolUseId } : {}),
		projectKey,
		workspaceKey: cwd,
		cwd,
		kind,
		occurredAt: adapter.ts,
		payload: envelope.payload,
		sourceHash,
		sensitivity: "normal",
	};
}

export function buildNormalizedEventFromClaudeHook(
	payload: Record<string, unknown>,
): Record<string, unknown> | null {
	return buildNormalizedHookEvent("claude-code", buildRawEventEnvelopeFromHook(payload));
}

export function buildNormalizedEventFromCodexHook(
	payload: Record<string, unknown>,
): Record<string, unknown> | null {
	return buildNormalizedHookEvent("codex", buildRawEventEnvelopeFromCodexHook(payload));
}

export function isNormalizedEventKind(value: string): boolean {
	return KINDS.has(value);
}

function requiredString(event: Record<string, unknown>, field: string): string {
	const value = event[field];
	if (typeof value !== "string" || value.trim().length === 0) {
		throw new Error(`event.${field} is required.`);
	}
	return value;
}

export function validateNormalizedEvent(
	event: Record<string, unknown>,
	schemaVersion = NORMALIZED_SCHEMA_VERSION,
): {
	eventId: string;
	idempotencyKey: string;
	agent: string;
	kind: string;
	sensitivity: "normal" | "private" | "secret";
	occurredAt: string;
	sourceHash: string;
} {
	if (Object.keys(event).some((field) => !FIELDS.has(field))) {
		throw new Error("event contains an unsupported field.");
	}
	if (!Object.hasOwn(event, "payload")) throw new Error("event.payload is required.");
	if (event.schemaVersion !== schemaVersion)
		throw new Error("event.schemaVersion is incompatible.");
	const eventId = requiredString(event, "eventId");
	const idempotencyKey = requiredString(event, "idempotencyKey");
	const agent = requiredString(event, "agent");
	const kind = requiredString(event, "kind");
	const sensitivity = requiredString(event, "sensitivity");
	const occurredAt = requiredString(event, "occurredAt");
	const sourceHash = requiredString(event, "sourceHash");
	for (const field of ["nativeSessionId", "projectKey", "workspaceKey", "cwd"]) {
		requiredString(event, field);
	}
	if (!AGENTS.has(agent)) throw new Error("event.agent is unsupported.");
	if (!isNormalizedEventKind(kind)) throw new Error("event.kind is unsupported.");
	if (!SENSITIVITIES.has(sensitivity)) throw new Error("event.sensitivity is unsupported.");
	if (!Number.isFinite(Date.parse(occurredAt))) throw new Error("event.occurredAt is invalid.");
	if (!/^[a-f0-9]{64}$/.test(sourceHash)) {
		throw new Error("event.sourceHash must be a SHA-256 hex digest.");
	}
	for (const field of [
		"agentInstanceId",
		"parentSessionId",
		"nativeTurnId",
		"nativeToolUseId",
		"branchKey",
		"gitHeadSha",
		"dirtyTreeFingerprint",
		"model",
	]) {
		if (event[field] !== undefined && typeof event[field] !== "string") {
			throw new Error(`event.${field} must be a string.`);
		}
	}
	if (
		event.nativeSequence !== undefined &&
		(!Number.isInteger(event.nativeSequence) || Number(event.nativeSequence) < 0)
	) {
		throw new Error("event.nativeSequence must be a non-negative integer.");
	}
	if (
		event.injectedContextIds !== undefined &&
		(!Array.isArray(event.injectedContextIds) ||
			!event.injectedContextIds.every((value) => typeof value === "string"))
	) {
		throw new Error("event.injectedContextIds must be an array of strings.");
	}
	return {
		eventId,
		idempotencyKey,
		agent,
		kind,
		sensitivity: sensitivity as "normal" | "private" | "secret",
		occurredAt,
		sourceHash,
	};
}

export function sealDegradedNormalizedEvent(
	event: Record<string, unknown>,
): Record<string, unknown> {
	const sealed: Record<string, unknown> = {};
	for (const field of DEGRADED_COPY_FIELDS) {
		if (Object.hasOwn(event, field)) sealed[field] = event[field];
	}
	for (const field of ["nativeSessionId", "projectKey", "workspaceKey", "cwd"] as const) {
		sealed[field] = redactedMetadataValue(event[field]);
	}
	sealed.payload = {};
	sealed.sensitivity = "secret";
	return sealed;
}
