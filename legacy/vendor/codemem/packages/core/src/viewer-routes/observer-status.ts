/**
 * Observer status route — GET /api/observer-status.
 *
 * Ports Python's viewer_routes/observer_status.py.
 * Returns observer runtime info, credential availability, and queue status.
 */

import { Hono } from "hono";
import { captureOnlyCapabilityProjection } from "../capability-manifest.js";
import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileUntrustedDestinationBoundary,
	type DestinationBoundaryV1,
	destinationBoundarySql,
} from "../destination-boundary.js";
import type { ObserverClient } from "../observer-client.js";
import type { RawEventSweeper } from "../raw-event-sweeper.js";
import type { MemoryStore } from "../store.js";
import { boundaryFilteredBacklogTotals } from "./raw-events.js";

type StoreFactory = () => MemoryStore;

export interface ObserverStatusDeps {
	getStore: StoreFactory;
	getSweeper: () => RawEventSweeper | null;
	getObserver?: () => ObserverClient | null;
	getCapabilitySnapshot?: () => Record<string, unknown>;
	destinationBoundary?: DestinationBoundaryV1;
}

function normalizeActiveObserver(active: ReturnType<ObserverClient["getStatus"]> | null) {
	if (!active) return null;
	return {
		...active,
		auth: {
			...active.auth,
			method: active.auth.type,
			token_present: active.auth.hasToken,
		},
	};
}

function capabilityObserverStatus(capability: Record<string, unknown>) {
	if (capability.providerEnabled !== true) return null;
	const provider = capability.summaryProvider;
	if (!provider || typeof provider !== "object" || Array.isArray(provider)) return null;
	const choice = provider as Record<string, unknown>;
	const credential = choice.credentialRef;
	const credentialKind =
		credential && typeof credential === "object" && !Array.isArray(credential)
			? (credential as Record<string, unknown>).kind
			: "none";
	let providerName: "anthropic" | "openai" | null = null;
	if (choice.wireProtocol === "anthropic_messages_v1") {
		providerName = "anthropic";
	} else if (choice.wireProtocol === "openai_chat_completions_v1") {
		providerName = "openai";
	}
	return {
		provider: providerName,
		model: typeof choice.modelId === "string" ? choice.modelId : null,
		runtime: "api_http",
		auth: { method: credentialKind, token_present: false },
	};
}

// error_type is written from Error.name (arbitrary TEXT, legacy rows worse);
// map it onto this closed enum instead of echoing stored values to the viewer.
function closedFailureType(raw: unknown): "auth_failed" | "provider_timeout" | "unexpected_error" {
	const value = String(raw ?? "");
	if (value === "ObserverAuthError") return "auth_failed";
	if (value === "TimeoutError" || value.toLowerCase().includes("timeout")) {
		return "provider_timeout";
	}
	return "unexpected_error";
}

/**
 * Latest failed flush batch, restricted to batches whose OWN event range the
 * destination boundary can see, projected onto a closed vocabulary. Never
 * returns stream or session identifiers, free-text messages, provider codes
 * (both TEXT columns are unbounded), or auth/model details.
 */
function latestVisibleFlushFailure(
	store: MemoryStore,
	boundary: DestinationBoundaryV1,
): Record<string, unknown> | null {
	const predicate = destinationBoundarySql(boundary, "events");
	const row = store.db
		.prepare(
			`SELECT batches.error_type, batches.attempt_count, batches.updated_at
			 FROM raw_event_flush_batches AS batches
			 WHERE batches.status IN ('error', 'failed', 'retry_exhausted')
			   AND EXISTS (
				SELECT 1 FROM raw_events AS events
				WHERE events.source = batches.source
				  AND events.stream_id = batches.stream_id
				  AND events.event_seq BETWEEN batches.start_event_seq AND batches.end_event_seq
				  AND ${predicate.clause}
			   )
			 ORDER BY batches.updated_at DESC LIMIT 1`,
		)
		.get(...predicate.params) as Record<string, unknown> | undefined;
	if (!row) return null;
	return {
		status: "error",
		error_type: closedFailureType(row.error_type),
		attempt_count: Number(row.attempt_count ?? 0),
		updated_at: typeof row.updated_at === "string" ? row.updated_at : null,
	};
}

function buildFailureImpact(
	latestFailure: Record<string, unknown> | null,
	queueTotals: { pending: number; sessions: number },
	authBackoff: { active: boolean; remainingS: number },
): string | null {
	if (!latestFailure) return null;
	if (authBackoff.active) {
		return `Queue retries paused for ~${authBackoff.remainingS}s after an observer auth failure.`;
	}
	if (queueTotals.pending > 0) {
		return `${queueTotals.pending} queued raw events across ${queueTotals.sessions} session(s) are waiting on a successful flush.`;
	}
	return "Failed flush batches are pending retry.";
}

export function observerStatusRoutes(deps?: ObserverStatusDeps) {
	const app = new Hono();
	// Queue totals go through the same boundary predicate as the raw-events
	// route, so restricted sessions never surface even as counts. No boundary
	// supplied → fail closed to the untrusted capture-only boundary.
	const boundary =
		deps?.destinationBoundary ??
		compileUntrustedDestinationBoundary({
			consumer: "viewer",
			configurationFingerprint: CAPTURE_ONLY_DESTINATION_FINGERPRINT,
		});

	app.get("/api/observer-status", (c) => {
		const store = deps?.getStore();
		const sweeper = deps?.getSweeper();
		const observer = deps?.getObserver?.() ?? null;
		const capability = deps?.getCapabilitySnapshot?.() ?? captureOnlyCapabilityProjection();

		// Stub fallback when store doesn't have the required surface (e.g. tests
		// with mock store). Boundary-filtered totals need a live db handle.
		if (!store || typeof store.db?.prepare !== "function") {
			return c.json({
				active: capabilityObserverStatus(capability),
				capability,
				available_credentials: {},
				latest_failure: null,
				queue: {
					pending: 0,
					sessions: 0,
					auth_backoff_active: false,
					auth_backoff_remaining_s: 0,
				},
			});
		}

		const queueTotals = boundaryFilteredBacklogTotals(store, boundary);
		const authBackoff = sweeper?.authBackoffStatus() ?? { active: false, remainingS: 0 };
		const latestFailure = latestVisibleFlushFailure(store, boundary);
		const active = deps?.getCapabilitySnapshot
			? capabilityObserverStatus(capability)
			: normalizeActiveObserver(observer?.getStatus() ?? null);
		const shouldShowFailure =
			latestFailure != null && (authBackoff.active || queueTotals.pending > 0);

		const failureWithImpact =
			shouldShowFailure && latestFailure
				? { ...latestFailure, impact: buildFailureImpact(latestFailure, queueTotals, authBackoff) }
				: null;

		return c.json({
			active,
			capability,
			available_credentials: {},
			latest_failure: failureWithImpact,
			queue: {
				...queueTotals,
				auth_backoff_active: authBackoff.active,
				auth_backoff_remaining_s: authBackoff.remainingS,
			},
		});
	});

	return app;
}
