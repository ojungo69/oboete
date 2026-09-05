import { PROCESSING_JOB_MAX_SOURCE_EVENTS } from "./capability-manifest.js";
import type { Database } from "./db.js";

export type RawEventSourceRangeInspection =
	| { status: "empty" }
	| { status: "source_gap" }
	| { status: "ready"; eventCount: number };

/** Inspect the exact bounded leading range used by raw-event job admission. */
export function inspectRawEventSourceRange(
	db: Database,
	input: {
		source: string;
		streamId: string;
		lastFlushedEventSeq: number;
		lastReceivedEventSeq: number;
	},
): RawEventSourceRangeInspection {
	const events = db
		.prepare(
			`SELECT event_seq, event_id, repository_identity
			 FROM raw_events
			 WHERE source = ? AND stream_id = ? AND event_seq > ? AND event_seq <= ?
			 ORDER BY event_seq
			 LIMIT ?`,
		)
		.all(
			input.source,
			input.streamId,
			input.lastFlushedEventSeq,
			input.lastReceivedEventSeq,
			PROCESSING_JOB_MAX_SOURCE_EVENTS,
		) as Array<{
		event_seq: number;
		event_id: string;
		repository_identity: string | null;
	}>;
	if (events.length === 0) {
		return input.lastReceivedEventSeq > input.lastFlushedEventSeq
			? { status: "source_gap" }
			: { status: "empty" };
	}
	const repositoryIdentity = events[0]?.repository_identity ?? null;
	const eventIds = new Set<string>();
	for (let index = 0; index < events.length; index++) {
		const event = events[index];
		if (!event || Number(event.event_seq) !== input.lastFlushedEventSeq + index + 1) {
			return { status: "source_gap" };
		}
		if (event.repository_identity !== repositoryIdentity || eventIds.has(event.event_id)) {
			return { status: "ready", eventCount: index };
		}
		eventIds.add(event.event_id);
	}
	return { status: "ready", eventCount: events.length };
}

/** Verify that one immutable processing-job range is retained exactly. */
export function hasExactRawEventSourceRange(
	db: Database,
	input: { source: string; streamId: string; startEventSeq: number; endEventSeq: number },
): boolean {
	if (
		!Number.isSafeInteger(input.startEventSeq) ||
		!Number.isSafeInteger(input.endEventSeq) ||
		input.startEventSeq < 0 ||
		input.startEventSeq > input.endEventSeq
	) {
		return false;
	}
	const expected = input.endEventSeq - input.startEventSeq + 1;
	if (!Number.isSafeInteger(expected)) return false;
	const range = db
		.prepare(
			`SELECT COUNT(*) AS event_count,
				COUNT(DISTINCT event_seq) AS sequence_count,
				SUM(CASE WHEN typeof(event_seq) = 'integer' THEN 1 ELSE 0 END)
					AS integer_sequence_count,
				COUNT(DISTINCT event_id) AS event_id_count,
				COUNT(DISTINCT CASE WHEN repository_identity IS NULL
					THEN 'null:' ELSE 'value:' || repository_identity END) AS repository_count,
				MIN(event_seq) AS first_seq, MAX(event_seq) AS last_seq
			 FROM raw_events
			 WHERE source = ? AND stream_id = ? AND event_seq BETWEEN ? AND ?`,
		)
		.get(input.source, input.streamId, input.startEventSeq, input.endEventSeq) as {
		event_count: number;
		sequence_count: number;
		integer_sequence_count: number;
		event_id_count: number;
		repository_count: number;
		first_seq: number | null;
		last_seq: number | null;
	};
	return (
		Number(range.event_count) === expected &&
		Number(range.sequence_count) === expected &&
		Number(range.integer_sequence_count) === expected &&
		Number(range.event_id_count) === expected &&
		Number(range.repository_count) === 1 &&
		Number(range.first_seq) === input.startEventSeq &&
		Number(range.last_seq) === input.endEventSeq
	);
}

/** Detect any missing sequence in a session's complete pending source range. */
export function hasRawEventSourceGap(
	db: Database,
	input: {
		source: string;
		streamId: string;
		lastFlushedEventSeq: number;
		lastReceivedEventSeq: number;
	},
): boolean {
	if (
		!Number.isSafeInteger(input.lastFlushedEventSeq) ||
		!Number.isSafeInteger(input.lastReceivedEventSeq) ||
		input.lastReceivedEventSeq < input.lastFlushedEventSeq
	) {
		throw new Error("Raw-event source frontier is invalid.");
	}
	const expected = input.lastReceivedEventSeq - input.lastFlushedEventSeq;
	if (expected === 0) return false;
	const row = db
		.prepare(
			`SELECT COUNT(*) AS count FROM raw_events
			 WHERE source = ? AND stream_id = ? AND event_seq > ? AND event_seq <= ?`,
		)
		.get(input.source, input.streamId, input.lastFlushedEventSeq, input.lastReceivedEventSeq) as {
		count: number;
	};
	return Number(row.count) !== expected;
}
