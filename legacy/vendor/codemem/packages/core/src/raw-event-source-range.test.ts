import Database from "better-sqlite3";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	hasExactRawEventSourceRange,
	hasRawEventSourceGap,
	inspectRawEventSourceRange,
} from "./raw-event-source-range.js";

describe("inspectRawEventSourceRange", () => {
	let db: Database.Database;

	beforeEach(() => {
		db = new Database(":memory:");
		db.exec(`CREATE TABLE raw_events (
			source TEXT NOT NULL,
			stream_id TEXT NOT NULL,
			event_seq INTEGER NOT NULL,
			event_id TEXT NOT NULL,
			repository_identity TEXT
		)`);
	});

	afterEach(() => db.close());

	function insert(eventSeq: number, eventId: string, repositoryIdentity = "repo-a") {
		db.prepare(
			`INSERT INTO raw_events(source, stream_id, event_seq, event_id, repository_identity)
			 VALUES ('opencode', 'stream', ?, ?, ?)`,
		).run(eventSeq, eventId, repositoryIdentity);
	}

	function inspect(lastReceivedEventSeq: number) {
		return inspectRawEventSourceRange(db, {
			source: "opencode",
			streamId: "stream",
			lastFlushedEventSeq: -1,
			lastReceivedEventSeq,
		});
	}

	it("distinguishes empty, missing, and bounded ready ranges", () => {
		expect(inspect(-1)).toEqual({ status: "empty" });
		expect(inspect(0)).toEqual({ status: "source_gap" });
		for (let eventSeq = 0; eventSeq < 101; eventSeq++) {
			insert(eventSeq, `event-${eventSeq}`);
		}
		expect(inspect(100)).toEqual({ status: "ready", eventCount: 100 });
		db.prepare("DELETE FROM raw_events WHERE event_seq = 100").run();
		insert(101, "event-101");
		expect(inspect(101)).toEqual({ status: "ready", eventCount: 100 });
		expect(
			hasRawEventSourceGap(db, {
				source: "opencode",
				streamId: "stream",
				lastFlushedEventSeq: -1,
				lastReceivedEventSeq: 101,
			}),
		).toBe(true);
	});

	it("checks an exact legacy range beyond the admission prefix limit", () => {
		for (let eventSeq = 0; eventSeq < 101; eventSeq++) insert(eventSeq, `event-${eventSeq}`);
		const exact = () =>
			hasExactRawEventSourceRange(db, {
				source: "opencode",
				streamId: "stream",
				startEventSeq: 0,
				endEventSeq: 100,
			});

		expect(exact()).toBe(true);
		db.prepare("UPDATE raw_events SET repository_identity = 'repo-b' WHERE event_seq = 100").run();
		expect(exact()).toBe(false);
		db.prepare(
			"UPDATE raw_events SET repository_identity = 'repo-a', event_id = 'event-0' WHERE event_seq = 100",
		).run();
		expect(exact()).toBe(false);
	});

	it("rejects a fractional sequence inside an otherwise complete exact range", () => {
		insert(0, "event-0");
		insert(0.5, "event-fractional");
		insert(2, "event-2");
		expect(
			hasExactRawEventSourceRange(db, {
				source: "opencode",
				streamId: "stream",
				startEventSeq: 0,
				endEventSeq: 2,
			}),
		).toBe(false);
	});

	it("stops at repository and repeated-ID boundaries but rejects sequence gaps", () => {
		insert(0, "event-0", "repo-a");
		insert(1, "event-1", "repo-b");
		expect(inspect(1)).toEqual({ status: "ready", eventCount: 1 });

		db.exec("DELETE FROM raw_events");
		insert(0, "event-0");
		insert(1, "event-0");
		expect(inspect(1)).toEqual({ status: "ready", eventCount: 1 });

		db.exec("DELETE FROM raw_events");
		insert(0, "event-0");
		insert(2, "event-2", "repo-b");
		expect(inspect(2)).toEqual({ status: "source_gap" });
	});

	it("does not inspect a gap beyond the requested range", () => {
		insert(0, "event-0");
		insert(1, "event-1");
		insert(3, "event-3");
		expect(inspect(1)).toEqual({ status: "ready", eventCount: 2 });
		expect(inspect(3)).toEqual({ status: "source_gap" });
	});

	it("detects gaps independently of admission prefix boundaries", () => {
		insert(0, "event-0", "repo-a");
		insert(1, "event-1", "repo-b");
		insert(3, "event-3", "repo-b");
		expect(inspect(3)).toEqual({ status: "ready", eventCount: 1 });
		expect(
			hasRawEventSourceGap(db, {
				source: "opencode",
				streamId: "stream",
				lastFlushedEventSeq: -1,
				lastReceivedEventSeq: 3,
			}),
		).toBe(true);

		db.exec("DELETE FROM raw_events");
		insert(0, "event-0");
		insert(1, "event-0");
		insert(3, "event-3");
		expect(inspect(3)).toEqual({ status: "ready", eventCount: 1 });
		expect(
			hasRawEventSourceGap(db, {
				source: "opencode",
				streamId: "stream",
				lastFlushedEventSeq: -1,
				lastReceivedEventSeq: 3,
			}),
		).toBe(true);
	});
});
