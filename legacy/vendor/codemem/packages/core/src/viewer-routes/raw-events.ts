import { Hono } from "hono";
import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileUntrustedDestinationBoundary,
	type DestinationBoundaryV1,
	destinationBoundarySql,
} from "../destination-boundary.js";
import { parseStrictInteger } from "../integers.js";
import type { MemoryStore } from "../store.js";

/**
 * Backlog totals restricted to events the destination boundary can see.
 * Shared by the raw-events and observer-status routes so both report the
 * same boundary-filtered counts.
 */
export function boundaryFilteredBacklogTotals(
	store: MemoryStore,
	boundary: DestinationBoundaryV1,
): { pending: number; sessions: number } {
	const predicate = destinationBoundarySql(boundary, "events");
	const row = store.db
		.prepare(
			`SELECT COUNT(DISTINCT sessions.source || char(0) || sessions.stream_id) AS sessions,
				COUNT(*) AS pending
			 FROM raw_events AS events
			 JOIN raw_event_sessions AS sessions
				ON sessions.source = events.source AND sessions.stream_id = events.stream_id
			 WHERE events.event_seq > sessions.last_flushed_event_seq AND ${predicate.clause}`,
		)
		.get(...predicate.params) as { sessions: number; pending: number } | undefined;
	return { pending: Number(row?.pending ?? 0), sessions: Number(row?.sessions ?? 0) };
}

export function rawEventReadRoutes(
	getStore: () => MemoryStore,
	destinationBoundary?: DestinationBoundaryV1,
) {
	const app = new Hono();
	const boundary =
		destinationBoundary ??
		compileUntrustedDestinationBoundary({
			consumer: "viewer",
			configurationFingerprint: CAPTURE_ONLY_DESTINATION_FINGERPRINT,
		});
	const predicate = destinationBoundarySql(boundary, "events");
	const totals = (store: MemoryStore) => boundaryFilteredBacklogTotals(store, boundary);

	app.get("/api/raw-events", (c) => c.json(totals(getStore())));

	app.get("/api/raw-events/status", (c) => {
		const store = getStore();
		const limit = parseStrictInteger(c.req.query("limit")) ?? 25;
		const rows = store.db
			.prepare(
				`SELECT sessions.source, sessions.stream_id, sessions.started_at,
					sessions.last_seen_ts_wall_ms, sessions.last_received_event_seq,
					sessions.last_flushed_event_seq, sessions.updated_at
				 FROM raw_event_sessions AS sessions
				 WHERE EXISTS (
					SELECT 1 FROM raw_events AS events
					WHERE events.source = sessions.source AND events.stream_id = sessions.stream_id
						AND events.event_seq > sessions.last_flushed_event_seq
						AND ${predicate.clause}
				 )
				 ORDER BY sessions.updated_at DESC LIMIT ?`,
			)
			.all(...predicate.params, limit) as Array<Record<string, unknown>>;
		const items = rows.map((row) => {
			const streamId = String(row.stream_id ?? row.opencode_session_id ?? "");
			return { ...row, session_stream_id: streamId, session_id: streamId };
		});
		return c.json({
			items,
			totals: totals(store),
			ingest: { available: false, mode: "daemon_rpc", max_body_bytes: 0 },
		});
	});

	return app;
}
