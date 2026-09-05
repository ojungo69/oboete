/**
 * Raw event flush orchestration — bridge between the sweeper and the ingest pipeline.
 *
 * Ports the core flush logic from codemem/raw_event_flush.py.
 *
 * Reads unflushed raw events for a session, creates a batch record,
 * builds an IngestPayload, runs it through the ingest pipeline,
 * and updates flush state on success (or records failure details).
 */

import { extractApplyPatchPaths, MUTATING_TOOL_NAMES } from "./apply-patch.js";
import { compileProviderDestinationBoundary } from "./destination-boundary.js";
import { extractAdapterEvent } from "./ingest-events.js";
import { type IngestOptions, ingest } from "./ingest-pipeline.js";
import {
	isTrivialRequest,
	normalizeAdapterEvents,
	normalizeEventsForSessionContext,
} from "./ingest-transcript.js";
import type { IngestPayload, SessionContext } from "./ingest-types.js";
import { ObserverAuthError } from "./observer-client.js";
import { type MemoryStore, ProcessingOutputLimitError, StaleRawEventClaimError } from "./store.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function truncateErrorMessage(message: string, limit = 280): string {
	const text = message.replace(/\s+/g, " ").trim();
	if (text.length <= limit) return text;
	return `${text.slice(0, limit - 3).trimEnd()}...`;
}

function providerDisplayName(provider: string | null | undefined): string {
	const normalized = (provider ?? "").trim().toLowerCase();
	if (normalized === "openai") return "OpenAI";
	if (normalized === "anthropic") return "Anthropic";
	if (normalized) return normalized.charAt(0).toUpperCase() + normalized.slice(1);
	return "Observer";
}

function isTimeoutLike(error: Error): boolean {
	return error.name === "TimeoutError" || error.message.toLowerCase().includes("timeout");
}

// Maps an internal failure onto a closed set of fixed template strings. The
// raw message is only ever CLASSIFIED — never embedded — so the result stays
// content-free for diagnostics.
function summarizeFlushFailure(exc: Error, provider: string | null | undefined): string {
	const providerTitle = providerDisplayName(provider);
	const rawMessage = String(exc.message ?? "")
		.trim()
		.toLowerCase();

	if (exc instanceof ObserverAuthError) {
		return `${providerTitle} authentication failed. Refresh credentials and retry.`;
	}
	if (isTimeoutLike(exc)) {
		return `${providerTitle} request timed out during raw-event processing.`;
	}
	if (
		rawMessage === "observer failed during raw-event flush" ||
		rawMessage === "observer produced no storable output for raw-event flush"
	) {
		return `${providerTitle} returned no usable output for raw-event processing.`;
	}
	if (rawMessage === "observer repair remained lossy during raw-event flush") {
		return `${providerTitle} returned structurally incomplete output that could not be repaired.`;
	}
	if (/parse|xml|json/i.test(rawMessage)) {
		return `${providerTitle} response could not be processed.`;
	}
	return `${providerTitle} processing failed during raw-event ingestion.`;
}

// ---------------------------------------------------------------------------
// Session context builder
// ---------------------------------------------------------------------------

/**
 * Build session context from raw events — extracts prompt count, tool count,
 * duration, files modified/read, and first user prompt.
 *
 * Port of build_session_context() from raw_event_flush.py.
 */
export function buildSessionContext(events: Record<string, unknown>[]): SessionContext {
	let promptCount = 0;
	let toolCount = 0;

	for (const e of events) {
		if (e.type === "user_prompt") promptCount++;
		if (e.type === "tool.execute.after") toolCount++;
	}

	const tsValues: number[] = [];
	for (const e of events) {
		const ts = e.timestamp_wall_ms;
		if (ts == null) continue;
		const num = Number(ts);
		if (!Number.isFinite(num)) continue;
		tsValues.push(num);
	}
	let durationMs = 0;
	if (tsValues.length > 0) {
		const firstTs = tsValues[0];
		if (firstTs == null) {
			throw new Error("Expected timestamp when tsValues is non-empty");
		}
		const restTs = tsValues.slice(1);
		let minTs = firstTs;
		let maxTs = firstTs;
		for (const v of restTs) {
			if (v < minTs) minTs = v;
			if (v > maxTs) maxTs = v;
		}
		durationMs = Math.max(0, maxTs - minTs);
	}

	const filesModified = new Set<string>();
	const filesRead = new Set<string>();
	for (const e of events) {
		if (e.type !== "tool.execute.after") continue;
		const tool = String(e.tool ?? "").toLowerCase();
		const args = e.args;
		if (args == null || typeof args !== "object") continue;
		const argsObj = args as Record<string, unknown>;

		// OpenCode's primary file-mutation tool is `apply_patch`, which does not
		// expose a `filePath` key — the paths live inside the `patchText` string
		// using the standard `*** Add File: <path>` / `*** Update File: <path>` /
		// `*** Delete File: <path>` markers. Extract those explicitly so the
		// session context reflects writes from `apply_patch` sessions.
		//
		// `*** Delete File:` entries are also recorded as `filesModified` because
		// session context tracks "files the session touched", not just writes —
		// a deletion is a mutation and downstream session-aware surfaces (pack,
		// injection) want to see deleted paths too.
		if (tool === "apply_patch") {
			const patchText = argsObj.patchText ?? argsObj.patch;
			if (typeof patchText === "string" && patchText) {
				for (const path of extractApplyPatchPaths(patchText)) {
					filesModified.add(path);
				}
			}
			continue;
		}

		// Support both OpenCode-style camelCase (`filePath`) and Claude Code-style
		// snake_case (`file_path`) tool input keys. Claude Code hook payloads use
		// `file_path` verbatim from the Claude Code schema.
		const filePath = argsObj.filePath ?? argsObj.file_path ?? argsObj.path;
		if (typeof filePath !== "string" || !filePath) continue;
		if (MUTATING_TOOL_NAMES.has(tool)) {
			filesModified.add(filePath);
		}
		if (tool === "read") filesRead.add(filePath);
	}

	let firstPrompt: string | undefined;
	for (const e of events) {
		if (e.type !== "user_prompt") continue;
		const text = e.prompt_text;
		if (typeof text === "string" && text.trim()) {
			firstPrompt = text.trim();
			break;
		}
	}

	return {
		firstPrompt,
		promptCount,
		toolCount,
		durationMs,
		filesModified: [...filesModified].sort(),
		filesRead: [...filesRead].sort(),
	};
}

function isTerminalLowSignalSession(events: Record<string, unknown>[]): boolean {
	const normalizedEvents = normalizeAdapterEvents(events);
	const allowedTopLevelTypes = new Set(["session.started", "session.idle", "session.ended"]);
	const allowedAdapterTypes = new Set(["session_start", "session_end"]);
	return normalizedEvents.every((event) => {
		const type = String(event.type ?? "");
		if (type === "user_prompt") {
			const text = typeof event.prompt_text === "string" ? event.prompt_text.trim() : "";
			return Boolean(text) && isTrivialRequest(text);
		}
		if (allowedTopLevelTypes.has(type)) return true;
		const adapter = extractAdapterEvent(event);
		const adapterEventType = adapter?.event_type;
		return typeof adapterEventType === "string" && allowedAdapterTypes.has(adapterEventType);
	});
}

// ---------------------------------------------------------------------------
// Main flush function
// ---------------------------------------------------------------------------

export interface FlushRawEventsOptions {
	opencodeSessionId: string;
	source?: string;
	cwd?: string | null;
	project?: string | null;
	startedAt?: string | null;
}

/**
 * Flush raw events for a single session through the ingest pipeline.
 *
 * 1. Reads unflushed raw events from the store
 * 2. Creates/claims a flush batch for idempotency
 * 3. Builds session context and IngestPayload
 * 4. Calls ingest()
 * 5. Updates flush state on success; records failure details on error
 *
 * Port of flush_raw_events() from raw_event_flush.py.
 */
export async function flushRawEvents(
	store: MemoryStore,
	ingestOpts: IngestOptions,
	opts: FlushRawEventsOptions,
): Promise<{ flushed: number; updatedState: number }> {
	let { source = "opencode", cwd, project, startedAt } = opts;
	const { opencodeSessionId } = opts;

	source = (source ?? "").trim().toLowerCase() || "opencode";

	// Resolve session metadata for missing fields
	const meta = store.rawEventSessionMeta(opencodeSessionId, source);
	cwd ??= (meta.cwd as string) ?? process.cwd();
	project ??= (meta.project as string) ?? null;
	startedAt ??= (meta.started_at as string) ?? null;

	const manifestFingerprint = ingestOpts.configurationFingerprint;
	const providerFingerprint = ingestOpts.providerFingerprint;
	const manifest = ingestOpts.capabilityManifest;
	if (!manifestFingerprint || !providerFingerprint || !manifest) {
		throw new Error("raw-event flush requires frozen manifest and provider fingerprints");
	}
	const admission = store.admitRawEventFlushJob({
		source,
		streamId: opencodeSessionId,
		manifestFingerprint,
		providerFingerprint,
	});
	if (
		admission.status === "no_events" ||
		admission.status === "capacity" ||
		admission.status === "source_gap" ||
		admission.jobId === undefined
	) {
		return { flushed: 0, updatedState: 0 };
	}
	const repositoryIdentity = store.rawEventFlushJobRepositoryIdentity(admission.jobId);
	const boundary = compileProviderDestinationBoundary(manifest, {
		repositoryIdentity,
		tlsPeerVerified: ingestOpts.providerTlsPeerVerified === true,
	});
	const claim = store.claimRawEventFlushJob({
		jobId: admission.jobId,
		manifestFingerprint,
		providerFingerprint,
		maxMemoryItemsPerDerivation: ingestOpts.resourceProfile?.maxMemoryItemsPerDerivation,
		manifest,
		boundary,
	});
	if (!claim) return { flushed: 0, updatedState: 0 };
	const capturedEvents = store.loadRawEventFlushJobEvents(claim);
	const sourceEventIds = store.rawEventFlushClaimSourceEventIds(claim);
	const projectedSourceSet = store.rawEventFlushClaimProjectedSourceSet(claim);
	const events = capturedEvents;
	const projectedSourceEventIds = events.map((event) => String(event.event_id));
	const startEventSeq = claim.startEventSeq;
	const lastEventSeq = claim.endEventSeq;
	if (events.length === 0) {
		store.completeRawEventFlushJobPrivacySkip({
			claim,
			sourceEventIds,
			projection: { eligibleSourceEventIds: [], omittedSourceEventIds: sourceEventIds },
			diagnostic: {
				version: 1,
				action: "skipped",
				reason:
					boundary.executionLocation === "local" && repositoryIdentity === null
						? "repository_unknown"
						: "all_restricted",
				destination: boundary.executionLocation,
				configurationFingerprint: boundary.configurationFingerprint,
				providerFingerprint: boundary.providerFingerprint,
				attemptFingerprint: claim.attemptFingerprint,
				nextAction: "none",
			},
		});
		return { flushed: sourceEventIds.length, updatedState: 1 };
	}

	// Build session context. Claude Code raw events arrive as `claude.hook`
	// with an adapter envelope; normalize them to the flat user_prompt /
	// tool.execute.after shapes before scanning so promptCount, toolCount,
	// firstPrompt, filesRead, and filesModified are populated correctly.
	const normalizedForContext = normalizeEventsForSessionContext(events);
	const sessionContext: SessionContext = buildSessionContext(normalizedForContext);
	sessionContext.opencodeSessionId = opencodeSessionId;
	sessionContext.source = source;
	sessionContext.streamId = opencodeSessionId;
	sessionContext.flusher = "raw_events";
	sessionContext.flushBatch = {
		batch_id: claim.jobId,
		start_event_seq: startEventSeq,
		end_event_seq: lastEventSeq,
		claim,
		source_event_ids: sourceEventIds,
		projected_source_event_ids: projectedSourceEventIds,
		projected_repository_identity: projectedSourceSet.repositoryIdentity,
		projected_sources: projectedSourceSet.sources.map((item) => ({
			ordinal: item.ordinal,
			redacted_payload: item.redactedPayload,
		})),
	};

	if (isTerminalLowSignalSession(events)) {
		store.completeRawEventFlushJobMemory(
			{
				claim,
				sourceEventIds,
				observedOutputCount: 0,
				diagnostic: {
					version: 1,
					action: "skipped",
					reason: "eligible_only",
					nextAction: "none",
				},
			},
			() => [],
		);
		return { flushed: sourceEventIds.length, updatedState: 1 };
	}

	// Build ingest payload
	const payload: IngestPayload = {
		cwd: cwd ?? undefined,
		project: project ?? undefined,
		startedAt: startedAt ?? new Date().toISOString(),
		events,
		sessionContext,
	};

	// Run ingest pipeline
	try {
		await ingest(payload, store, ingestOpts);
	} catch (exc) {
		// Record failure details on the batch
		const err = exc instanceof Error ? exc : new Error(String(exc));
		const status = ingestOpts.observer?.getStatus?.();
		const provider = status?.provider as string | undefined;
		const message = truncateErrorMessage(summarizeFlushFailure(err, provider));
		let safeErrorCode = "output_invalid";
		if (err instanceof ObserverAuthError) safeErrorCode = "provider_auth_failed";
		else if (isTimeoutLike(err)) safeErrorCode = "provider_unavailable";
		else if (err instanceof ProcessingOutputLimitError) safeErrorCode = "output_limit_exceeded";
		try {
			store.failRawEventFlushJob({
				jobId: claim.jobId,
				claimGeneration: claim.claimGeneration,
				attemptFingerprint: claim.attemptFingerprint,
				safeErrorCode,
				diagnostic: {
					version: 1,
					action: "failed",
					reason: safeErrorCode,
					destination: "unknown",
					configurationFingerprint: claim.manifestFingerprint,
					providerFingerprint: claim.providerFingerprint,
					attemptFingerprint: claim.attemptFingerprint,
					nextAction:
						safeErrorCode === "provider_auth_failed" ? "configure_credential" : "confirm_retry",
				},
			});
		} catch (error_) {
			if (error_ instanceof StaleRawEventClaimError) throw exc;
			throw error_;
		}
		// Preserve the bounded legacy diagnostic columns for existing status surfaces.
		try {
			store.recordRawEventFlushBatchDiagnostic(claim.jobId, {
				message,
				errorType: err instanceof ObserverAuthError ? "ObserverAuthError" : err.name,
				observerProvider: provider ?? null,
				observerModel: status?.model ?? null,
				observerRuntime: status?.runtime ?? null,
				observerAuthSource: null,
				observerAuthType: null,
				observerErrorCode: status?.lastError?.code ?? null,
				observerErrorMessage: null,
			});
		} catch {
			// The canonical job failure above is authoritative; legacy diagnostics are best-effort.
		}
		throw exc;
	}

	// Ingest commits memory/job/frontier through the Store-owned completion transaction.
	return { flushed: sourceEventIds.length, updatedState: 1 };
}
