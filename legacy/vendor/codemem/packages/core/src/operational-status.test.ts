import Database from "better-sqlite3";
import { describe, expect, it } from "vitest";
import { rawEventPayloadDigest, tableExists } from "./db.js";
import { collectOperationalStatus } from "./operational-status.js";
import { initTestSchema } from "./test-utils.js";

describe("collectOperationalStatus", () => {
	it("reports content-free source gaps with a closed recovery action", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			const sensitive = "source-gap-private-path";
			db.prepare(
				`INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				 ) VALUES ('opencode', ?, ?, 2, -1, '2026-09-01T00:00:00Z')`,
			).run(sensitive, sensitive);
			for (const eventSeq of [0, 2]) {
				db.prepare(
					`INSERT INTO raw_events(
						source, stream_id, opencode_session_id, event_id, event_seq,
						event_type, payload_json, created_at, payload_digest
					 ) VALUES ('opencode', ?, ?, ?, ?, 'message', ?, '2026-09-01T00:00:00Z', ?)`,
				).run(
					sensitive,
					sensitive,
					`${sensitive}-${eventSeq}`,
					eventSeq,
					JSON.stringify({ path: sensitive }),
					rawEventPayloadDigest({ path: sensitive }),
				);
			}
			db.prepare(
				`INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, attempt_count, created_at, updated_at
				 ) VALUES (1, 'opencode', ?, ?, 0, 2, 'raw_events_v1', 'retry_exhausted', 3,
					'2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')`,
			).run(sensitive, sensitive);

			const result = collectOperationalStatus(db);

			expect(result.raw_events.source_gaps).toBe(1);
			expect(result.processing_jobs.exhausted).toBe(1);
			expect(result.processing_jobs.retry_exhausted_job_ids).toEqual([]);
			expect(result.processing_jobs.next_action).toBe("upgrade_runtime");
			expect(JSON.stringify(result)).not.toContain(sensitive);
		} finally {
			db.close();
		}
	});

	it("reports a source gap when the entire pending tail is missing", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			const sensitive = "fully-pruned-private-stream";
			db.prepare(
				`INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				 ) VALUES ('opencode', ?, ?, 2, -1, '2026-09-01T00:00:00Z')`,
			).run(sensitive, sensitive);

			const result = collectOperationalStatus(db);

			expect(result.raw_events.pending).toBe(0);
			expect(result.raw_events.source_gaps).toBe(1);
			expect(result.processing_jobs.next_action).toBe("upgrade_runtime");
			expect(JSON.stringify(result)).not.toContain(sensitive);
		} finally {
			db.close();
		}
	});

	it("requires an upgrade when an advanced legacy exhausted range lost its source", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			db.prepare(
				`INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				 ) VALUES ('codex', 'advanced-missing', 'advanced-missing', 0, 0,
					'2026-09-01T00:00:00Z')`,
			).run();
			db.prepare(
				`INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, attempt_count, legacy_recovery_state,
					frontier_already_advanced, created_at, updated_at
				 ) VALUES (1, 'codex', 'advanced-missing', 'advanced-missing', 0, 0,
					'raw_events_v1', 'retry_exhausted', 3, 'complete_range', 1,
					'2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')`,
			).run();

			const result = collectOperationalStatus(db);

			expect(result.raw_events).toMatchObject({
				available: true,
				pending: 0,
				source_gaps: 0,
			});
			expect(result.processing_jobs).toMatchObject({
				exhausted: 1,
				retry_exhausted_job_ids: [],
				next_action: "upgrade_runtime",
			});
		} finally {
			db.close();
		}
	});

	it("preserves the pending count when the optional source-gap scan is unavailable", () => {
		const db = new Database(":memory:");
		try {
			db.exec(`
				CREATE TABLE raw_event_sessions (
					source TEXT NOT NULL,
					stream_id TEXT NOT NULL,
					last_received_event_seq INTEGER NOT NULL,
					last_flushed_event_seq INTEGER NOT NULL
				);
				CREATE TABLE raw_events (
					source TEXT NOT NULL,
					stream_id TEXT NOT NULL,
					event_seq INTEGER NOT NULL,
					event_id TEXT NOT NULL,
					repository_identity TEXT
				);
				CREATE TABLE raw_event_flush_batches (id INTEGER PRIMARY KEY);
				INSERT INTO raw_event_sessions VALUES ('opencode', 'partial', 0, -1);
				INSERT INTO raw_events VALUES ('opencode', 'partial', 0, 'event-0', NULL);
			`);

			const result = collectOperationalStatus(db);

			expect(result.raw_events).toMatchObject({ available: false, pending: 1, source_gaps: 0 });
			expect(result.processing_jobs.next_action).toBe("upgrade_runtime");
		} finally {
			db.close();
		}
	});

	it("collects fixed aggregate operational evidence without exposing rows", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			if (tableExists(db, "memory_vectors")) db.exec("DROP TABLE memory_vectors");
			db.prepare(
				`INSERT INTO maintenance_jobs(kind, title, status, updated_at, error)
				 VALUES ('vector_model_migration', 'Vectors', 'failed', '2026-08-11T10:00:00Z', 'private vector failure')`,
			).run();
			db.prepare(
				`INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				 ) VALUES ('opencode', 'session-1', 'session-1', 3, 1, '2026-08-11T10:00:00Z')`,
			).run();
			for (const eventSeq of [2, 3]) {
				db.prepare(
					`INSERT INTO raw_events(source, stream_id, opencode_session_id, event_id, event_seq, event_type, payload_json, created_at, payload_digest)
					 VALUES ('opencode', 'session-1', 'session-1', ?, ?, 'message', '{}', '2026-08-11T10:00:00Z', ?)`,
				).run(`event-${eventSeq}`, eventSeq, rawEventPayloadDigest({}));
			}
			db.prepare(
				`INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, observer_error_code, created_at, updated_at
				 ) VALUES (1, 'opencode', 'session-1', 'session-1', 2, 3, 'v1', 'retry_exhausted',
					'auth_failure', '2026-08-11T10:00:00Z', '2026-08-11T10:00:00Z')`,
			).run();
			db.prepare(
				`INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, observer_error_code, created_at, updated_at
				 ) VALUES (2, 'opencode', 'session-1', 'session-1', 4, 4, 'v1', 'failed',
					'rate_limited', '2026-08-11T10:00:00Z', '2026-08-11T10:00:00Z')`,
			).run();

			const result = collectOperationalStatus(db);

			expect(result).toEqual({
				capability: null,
				maintenance: { state: "idle", running: 0, failed: 0 },
				semantic_index: { state: "failed", vector_table_present: false },
				raw_events: { available: true, pending: 2, source_gaps: 0, failed_batches: 1 },
				processing_jobs: {
					capacity: 25,
					uncompleted: 2,
					processing: 0,
					failed: 1,
					exhausted: 1,
					pending_grants: 0,
					max_attempt: 0,
					legacy_unrecoverable: 0,
					retry_exhausted_job_ids: [1],
					next_action: "activate_valid_manifest",
				},
				observer: { available: true, failed_batches: 1, backoff_batches: 1 },
			});
			expect(JSON.stringify(result)).not.toContain("private");
			expect(
				collectOperationalStatus(db, {
					capability: {
						configurationFingerprint: `sha256:${"a".repeat(64)}`,
						summaryProvider: { providerFingerprint: `sha256:${"b".repeat(64)}` },
					},
				}).processing_jobs.next_action,
			).toBe("confirm_retry");
			const afterRecentWindow = collectOperationalStatus(db, {
				recentFailureCutoff: "2026-08-12T10:00:00Z",
			});
			expect(afterRecentWindow.raw_events.failed_batches).toBe(0);
			expect(afterRecentWindow.observer).toEqual({
				available: true,
				failed_batches: 0,
				backoff_batches: 1,
			});
		} finally {
			db.close();
		}
	});

	it("tolerates optional tables being absent", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			if (tableExists(db, "memory_vectors")) db.exec("DROP TABLE memory_vectors");
			for (const table of ["maintenance_jobs", "raw_event_flush_batches"]) {
				db.exec(`DROP TABLE ${table}`);
			}

			expect(collectOperationalStatus(db, { embeddingDisabled: true })).toEqual({
				capability: null,
				maintenance: { state: "unknown", running: 0, failed: 0 },
				semantic_index: { state: "degraded", vector_table_present: false },
				raw_events: { available: false, pending: 0, source_gaps: 0, failed_batches: 0 },
				processing_jobs: {
					capacity: 25,
					uncompleted: 0,
					processing: 0,
					failed: 0,
					exhausted: 0,
					pending_grants: 0,
					max_attempt: 0,
					legacy_unrecoverable: 0,
					retry_exhausted_job_ids: [],
					next_action: "upgrade_runtime",
				},
				observer: { available: false, failed_batches: 0, backoff_batches: 0 },
			});
		} finally {
			db.close();
		}
	});

	it("bounds and sorts retry-exhausted job IDs for doctor recovery", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			const insert = db.prepare(
				`INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, created_at, updated_at
				 ) VALUES (?, 'codex', ?, ?, 0, 0, 'v1', 'retry_exhausted', ?, ?)`,
			);
			const insertSession = db.prepare(
				`INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				 ) VALUES ('codex', ?, ?, 0, -1, 'now')`,
			);
			const insertEvent = db.prepare(
				`INSERT INTO raw_events(
					source, stream_id, opencode_session_id, event_id, event_seq, event_type,
					payload_json, created_at, payload_digest
				 ) VALUES ('codex', ?, ?, ?, 0, 'message', '{}', 'now', ?)`,
			);
			for (let id = 25; id >= 1; id--) {
				const streamId = `stream-${id}`;
				insertSession.run(streamId, streamId);
				insertEvent.run(streamId, streamId, `${streamId}-0`, rawEventPayloadDigest({}));
				insert.run(id, streamId, streamId, "now", "now");
			}

			expect(collectOperationalStatus(db).processing_jobs.retry_exhausted_job_ids).toEqual(
				Array.from({ length: 25 }, (_, index) => index + 1),
			);
		} finally {
			db.close();
		}
	});

	it("caps source-gap inspection at processing capacity", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			const insert = db.prepare(
				`INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				 ) VALUES ('opencode', ?, ?, 0, -1, ?)`,
			);
			for (let index = 0; index < 30; index++) {
				const streamId = `fully-pruned-${index}`;
				insert.run(streamId, streamId, `2026-09-01T00:00:${String(index).padStart(2, "0")}Z`);
			}

			expect(collectOperationalStatus(db).raw_events.source_gaps).toBe(25);
		} finally {
			db.close();
		}
	});

	it.each([
		["configured", { mode: "configured", embeddingProvider: { state: "disabled" } }],
		["capture-only", { mode: "capture_only" }],
	])("reports semantic degradation for %s capability", (_mode, capability) => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			db.prepare(
				`INSERT INTO maintenance_jobs(kind, title, status, updated_at)
				 VALUES ('vector_model_migration', 'Vectors', 'pending', '2026-08-31T00:00:00Z')`,
			).run();

			expect(
				collectOperationalStatus(db, {
					embeddingDisabled: false,
					capability,
				}).semantic_index.state,
			).toBe("degraded");
		} finally {
			db.close();
		}
	});

	it("lets the emergency embedding disable override an enabled capability", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);

			expect(
				collectOperationalStatus(db, {
					embeddingDisabled: true,
					capability: { mode: "configured", embeddingProvider: { state: "enabled" } },
				}).semantic_index.state,
			).toBe("degraded");
		} finally {
			db.close();
		}
	});
});
