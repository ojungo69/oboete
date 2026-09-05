import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileUntrustedDestinationBoundary,
} from "./destination-boundary.js";
import type { ObserverClient } from "./observer-client.js";
import type { RawEventSweeper } from "./raw-event-sweeper.js";
import type { MemoryStore } from "./store.js";
import { configReadRoutes } from "./viewer-routes/config.js";
import { memoryRoutes } from "./viewer-routes/memory.js";
import { observerStatusRoutes } from "./viewer-routes/observer-status.js";
import { rawEventReadRoutes } from "./viewer-routes/raw-events.js";
import { statsRoutes } from "./viewer-routes/stats.js";

const COLLECTION_PATHS: Record<string, string> = {
	sessions: "/api/sessions",
	projects: "/api/projects",
	memories: "/api/observations",
	observations: "/api/observations",
	summaries: "/api/summaries",
	session: "/api/session",
	memory: "/api/memory",
	artifacts: "/api/artifacts",
	"raw-events": "/api/raw-events",
	"raw-events-status": "/api/raw-events/status",
	stats: "/api/stats",
	runtime: "/api/runtime",
	usage: "/api/usage",
	"observer-status": "/api/observer-status",
	config: "/api/config",
};

export type ViewerReadHandler = (
	body: Record<string, unknown>,
) => Promise<{ status: number; body: unknown }>;

export function createViewerReadHandler(deps: {
	store: MemoryStore;
	sweeper?: RawEventSweeper | null;
	observer?: ObserverClient | null;
	capability: Record<string, unknown>;
}): ViewerReadHandler {
	const destinationBoundary = compileUntrustedDestinationBoundary({
		consumer: "viewer",
		configurationFingerprint:
			typeof deps.capability.configurationFingerprint === "string"
				? deps.capability.configurationFingerprint
				: CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	});
	const app = memoryRoutes(() => deps.store, destinationBoundary);
	app.route(
		"/",
		statsRoutes(() => deps.store, destinationBoundary),
	);
	app.route(
		"/",
		rawEventReadRoutes(() => deps.store, destinationBoundary),
	);
	app.route(
		"/",
		observerStatusRoutes({
			getStore: () => deps.store,
			getSweeper: () => deps.sweeper ?? null,
			getObserver: () => deps.observer ?? null,
			getCapabilitySnapshot: () => deps.capability,
			destinationBoundary,
		}),
	);
	app.route("/", configReadRoutes({ getCapabilitySnapshot: () => deps.capability }));

	return async (body) => {
		const path = COLLECTION_PATHS[String(body.collection ?? "")];
		if (!path) return { status: 400, body: { error: "unsupported collection" } };
		const query = new URLSearchParams();
		for (const [rpcName, queryName] of [
			["limit", "limit"],
			["offset", "offset"],
			["project", "project"],
			["kind", "kind"],
			["scope", "scope"],
			["sessionId", "session_id"],
		] as const) {
			const value = body[rpcName];
			if (typeof value === "string" || typeof value === "number") {
				query.set(queryName, String(value));
			}
		}
		const querySuffix = query.size > 0 ? `?${query}` : "";
		const response = await app.request(`${path}${querySuffix}`);
		const text = await response.text();
		let payload: unknown = {};
		try {
			payload = text ? JSON.parse(text) : {};
		} catch {
			payload = { error: "invalid daemon viewer response" };
		}
		return { status: response.status, body: payload };
	};
}
