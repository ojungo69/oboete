import { PROCESSING_JOB_CAPACITY } from "./capability-manifest.js";
import { columnExists, type Database, tableExists } from "./db.js";
import { hasExactRawEventSourceRange, hasRawEventSourceGap } from "./raw-event-source-range.js";
import { VECTOR_MODEL_MIGRATION_JOB } from "./vector-migration.js";

/** Doctor/status inspects at most one global processing-capacity window. */
export const OPERATIONAL_SOURCE_GAP_SCAN_LIMIT = PROCESSING_JOB_CAPACITY;

export type OperationalMaintenanceState = "idle" | "running" | "failed" | "unknown";
export type OperationalSemanticState = "healthy" | "pending" | "degraded" | "failed" | "unknown";
export type OperationalNextAction =
	| "none"
	| "activate_valid_manifest"
	| "configure_credential"
	| "wait_for_capacity"
	| "confirm_retry"
	| "restart_daemon"
	| "upgrade_runtime";

export interface OperationalProcessingJobs {
	capacity: number;
	uncompleted: number;
	processing: number;
	failed: number;
	exhausted: number;
	pending_grants: number;
	max_attempt: number;
	legacy_unrecoverable: number;
	retry_exhausted_job_ids: number[];
	next_action: OperationalNextAction;
}

export interface OperationalStatusSnapshot {
	capability: Record<string, unknown> | null;
	maintenance: {
		state: OperationalMaintenanceState;
		running: number;
		failed: number;
	};
	semantic_index: {
		state: OperationalSemanticState;
		vector_table_present: boolean;
	};
	raw_events: {
		available: boolean;
		pending: number;
		source_gaps: number;
		failed_batches: number;
	};
	processing_jobs: OperationalProcessingJobs;
	observer: {
		available: boolean;
		failed_batches: number;
		backoff_batches: number;
	};
}

function count(row: { count?: unknown } | undefined): number {
	const value = Number(row?.count ?? 0);
	return Number.isFinite(value) && value > 0 ? Math.trunc(value) : 0;
}

function aggregateCount(
	db: Database,
	table: string,
	query: string,
	params: unknown[] = [],
): number | null {
	if (!tableExists(db, table)) return null;
	try {
		return count(db.prepare(query).get(...params) as { count?: unknown } | undefined);
	} catch {
		return null;
	}
}

function collectSourceGapCount(db: Database): number {
	const streams = db
		.prepare(
			`SELECT s.source, s.stream_id, s.last_flushed_event_seq, s.last_received_event_seq
			 FROM raw_event_sessions s
			 WHERE s.last_received_event_seq > s.last_flushed_event_seq
			 ORDER BY s.updated_at, s.source, s.stream_id
			 LIMIT ?`,
		)
		.all(OPERATIONAL_SOURCE_GAP_SCAN_LIMIT) as Array<{
		source: string;
		stream_id: string;
		last_flushed_event_seq: number;
		last_received_event_seq: number;
	}>;
	let sourceGaps = 0;
	for (const stream of streams) {
		if (
			hasRawEventSourceGap(db, {
				source: stream.source,
				streamId: stream.stream_id,
				lastFlushedEventSeq: Number(stream.last_flushed_event_seq),
				lastReceivedEventSeq: Number(stream.last_received_event_seq),
			})
		) {
			sourceGaps++;
		}
	}
	return sourceGaps;
}

function collectMaintenance(db: Database): OperationalStatusSnapshot["maintenance"] {
	if (!tableExists(db, "maintenance_jobs")) {
		return { state: "unknown", running: 0, failed: 0 };
	}
	try {
		const row = db
			.prepare(
				`SELECT
					SUM(CASE WHEN status IN ('pending', 'running') THEN 1 ELSE 0 END) AS running,
					SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed
				 FROM maintenance_jobs
				 WHERE kind <> ?`,
			)
			.get(VECTOR_MODEL_MIGRATION_JOB) as { running?: unknown; failed?: unknown } | undefined;
		const running = count({ count: row?.running });
		const failed = count({ count: row?.failed });
		return { state: failed > 0 ? "failed" : running > 0 ? "running" : "idle", running, failed };
	} catch {
		return { state: "unknown", running: 0, failed: 0 };
	}
}

function collectSemanticIndex(
	db: Database,
	embeddingDisabled: boolean,
): OperationalStatusSnapshot["semantic_index"] {
	const vectorTablePresent = tableExists(db, "memory_vectors");
	if (!tableExists(db, "maintenance_jobs")) {
		return {
			state: embeddingDisabled || !vectorTablePresent ? "degraded" : "unknown",
			vector_table_present: vectorTablePresent,
		};
	}
	try {
		const row = db
			.prepare("SELECT status FROM maintenance_jobs WHERE kind = ?")
			.get(VECTOR_MODEL_MIGRATION_JOB) as { status?: unknown } | undefined;
		const status = typeof row?.status === "string" ? row.status : null;
		const state: OperationalSemanticState =
			status === "failed"
				? "failed"
				: embeddingDisabled || !vectorTablePresent
					? "degraded"
					: status === "pending" || status === "running"
						? "pending"
						: "healthy";
		return { state, vector_table_present: vectorTablePresent };
	} catch {
		return {
			state: embeddingDisabled || !vectorTablePresent ? "degraded" : "unknown",
			vector_table_present: vectorTablePresent,
		};
	}
}

function collectRawEvents(
	db: Database,
	recentFailureCutoff?: string,
): OperationalStatusSnapshot["raw_events"] {
	const sessionsTablePresent = tableExists(db, "raw_event_sessions");
	const eventsTablePresent = tableExists(db, "raw_events");
	const batchesTablePresent = tableExists(db, "raw_event_flush_batches");
	let available = sessionsTablePresent && eventsTablePresent && batchesTablePresent;
	let pending = 0;
	let sourceGaps = 0;
	if (sessionsTablePresent && eventsTablePresent) {
		try {
			pending = count(
				db
					.prepare(
						`SELECT COALESCE(SUM(max_seq - last_flushed_event_seq), 0) AS count
						 FROM (
							SELECT s.last_flushed_event_seq,
								(SELECT MAX(e.event_seq)
								 FROM raw_events e
								 WHERE e.source = s.source AND e.stream_id = s.stream_id) AS max_seq
							FROM raw_event_sessions s
						 )
						 WHERE max_seq > last_flushed_event_seq`,
					)
					.get() as { count?: unknown } | undefined,
			);
		} catch {
			pending = 0;
			available = false;
		}
		try {
			sourceGaps = collectSourceGapCount(db);
		} catch {
			sourceGaps = 0;
			available = false;
		}
	}
	return {
		available,
		pending,
		source_gaps: sourceGaps,
		failed_batches:
			aggregateCount(
				db,
				"raw_event_flush_batches",
				recentFailureCutoff
					? "SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE status = 'retry_exhausted' AND updated_at >= ?"
					: "SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE status = 'retry_exhausted'",
				recentFailureCutoff ? [recentFailureCutoff] : [],
			) ?? 0,
	};
}

function collectProcessingJobs(
	db: Database,
	hasRetryTarget: boolean,
	sourceGaps: number,
	rawEventsAvailable: boolean,
): OperationalProcessingJobs {
	const empty: OperationalProcessingJobs = {
		capacity: PROCESSING_JOB_CAPACITY,
		uncompleted: 0,
		processing: 0,
		failed: 0,
		exhausted: 0,
		pending_grants: 0,
		max_attempt: 0,
		legacy_unrecoverable: 0,
		retry_exhausted_job_ids: [],
		next_action: "upgrade_runtime",
	};
	if (!tableExists(db, "raw_event_flush_batches")) return empty;
	try {
		const row = db
			.prepare(
				`SELECT
					SUM(CASE WHEN status IN ('queued', 'processing', 'failed', 'retry_exhausted') THEN 1 ELSE 0 END) AS uncompleted,
					SUM(CASE WHEN status = 'processing' THEN 1 ELSE 0 END) AS processing,
					SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed,
					SUM(CASE WHEN status = 'retry_exhausted' THEN 1 ELSE 0 END) AS exhausted,
					SUM(CASE WHEN resume_grant_state = 'pending' THEN 1 ELSE 0 END) AS pending_grants,
					COALESCE(MAX(attempt_count), 0) AS max_attempt,
					SUM(CASE WHEN completion_disposition = 'legacy_unrecoverable' THEN 1 ELSE 0 END) AS legacy_unrecoverable
				 FROM raw_event_flush_batches`,
			)
			.get() as Record<string, unknown> | undefined;
		const uncompleted = count({ count: row?.uncompleted });
		const processing = count({ count: row?.processing });
		const failed = count({ count: row?.failed });
		const exhausted = count({ count: row?.exhausted });
		const pendingGrants = count({ count: row?.pending_grants });
		const maxAttempt = count({ count: row?.max_attempt });
		const legacyUnrecoverable = count({ count: row?.legacy_unrecoverable });
		const retryExhaustedJobs = db
			.prepare(
				`SELECT id, source, stream_id, start_event_seq, end_event_seq
				 FROM raw_event_flush_batches
				 WHERE status = 'retry_exhausted' ORDER BY id LIMIT ?`,
			)
			.all(PROCESSING_JOB_CAPACITY) as Array<{
			id: number;
			source: string;
			stream_id: string;
			start_event_seq: number;
			end_event_seq: number;
		}>;
		let retryExhaustedJobIds =
			rawEventsAvailable && sourceGaps === 0
				? retryExhaustedJobs
						.filter((job) =>
							hasExactRawEventSourceRange(db, {
								source: job.source,
								streamId: job.stream_id,
								startEventSeq: Number(job.start_event_seq),
								endEventSeq: Number(job.end_event_seq),
							}),
						)
						.map((job) => Number(job.id))
				: [];
		const hasUnrecoverableExhaustedRange = exhausted > retryExhaustedJobIds.length;
		let nextAction: OperationalNextAction = "none";
		if (
			!rawEventsAvailable ||
			sourceGaps > 0 ||
			legacyUnrecoverable > 0 ||
			hasUnrecoverableExhaustedRange
		)
			nextAction = "upgrade_runtime";
		else if (exhausted > 0)
			nextAction = hasRetryTarget ? "confirm_retry" : "activate_valid_manifest";
		else if (uncompleted >= PROCESSING_JOB_CAPACITY) nextAction = "wait_for_capacity";
		if (nextAction === "upgrade_runtime") retryExhaustedJobIds = [];
		return {
			capacity: PROCESSING_JOB_CAPACITY,
			uncompleted,
			processing,
			failed,
			exhausted,
			pending_grants: pendingGrants,
			max_attempt: maxAttempt,
			legacy_unrecoverable: legacyUnrecoverable,
			retry_exhausted_job_ids: retryExhaustedJobIds,
			next_action: nextAction,
		};
	} catch {
		return empty;
	}
}

function collectObserver(
	db: Database,
	recentFailureCutoff?: string,
): OperationalStatusSnapshot["observer"] {
	const available =
		tableExists(db, "raw_event_flush_batches") &&
		columnExists(db, "raw_event_flush_batches", "observer_error_code");
	if (!available) return { available: false, failed_batches: 0, backoff_batches: 0 };
	return {
		available,
		failed_batches:
			aggregateCount(
				db,
				"raw_event_flush_batches",
				recentFailureCutoff
					? `SELECT COUNT(*) AS count FROM raw_event_flush_batches
					 WHERE status = 'retry_exhausted' AND updated_at >= ?
					   AND observer_error_code IS NOT NULL
					   AND TRIM(observer_error_code) != ''`
					: `SELECT COUNT(*) AS count FROM raw_event_flush_batches
					 WHERE status = 'retry_exhausted'
					   AND observer_error_code IS NOT NULL
					   AND TRIM(observer_error_code) != ''`,
				recentFailureCutoff ? [recentFailureCutoff] : [],
			) ?? 0,
		backoff_batches:
			aggregateCount(
				db,
				"raw_event_flush_batches",
				`SELECT COUNT(*) AS count FROM raw_event_flush_batches
				 WHERE status = 'failed'
				   AND observer_error_code IS NOT NULL
				   AND TRIM(observer_error_code) != ''`,
			) ?? 0,
	};
}

/** Collect bounded operational aggregates from an already-open database without writing schema. */
export function collectOperationalStatus(
	db: Database,
	options: {
		embeddingDisabled?: boolean;
		recentFailureCutoff?: string;
		capability?: Record<string, unknown>;
	} = {},
): OperationalStatusSnapshot {
	const embeddingDisabled =
		options.embeddingDisabled === true ||
		(options.capability !== undefined &&
			(options.capability.embeddingProvider as { state?: unknown } | null | undefined)?.state !==
				"enabled");
	const hasRetryTarget =
		typeof options.capability?.configurationFingerprint === "string" &&
		typeof (options.capability.summaryProvider as { providerFingerprint?: unknown } | undefined)
			?.providerFingerprint === "string";
	const rawEvents = collectRawEvents(db, options.recentFailureCutoff);
	return {
		capability: options.capability ?? null,
		maintenance: collectMaintenance(db),
		semantic_index: collectSemanticIndex(db, embeddingDisabled),
		raw_events: rawEvents,
		processing_jobs: collectProcessingJobs(
			db,
			hasRetryTarget,
			rawEvents.source_gaps,
			rawEvents.available,
		),
		observer: collectObserver(db, options.recentFailureCutoff),
	};
}
