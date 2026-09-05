import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { openTestMemoryStore } from "../test-utils.js";
import type { SensitivityV1 } from "../types.js";
import { observerStatusRoutes } from "./observer-status.js";

const frozenStatus = {
	configurationFingerprint:
		"sha256:2a5a5d2d3803d8f2dc2767981cbbf4f77cffc3aae8cebdc9d310e7645b27d53d",
	summaryProvider: {
		providerFingerprint: "sha256:d184deae938722877e017d85ab382a4f72c287857bf0f346f483263680635ede",
		wireProtocol: "openai_chat_completions_v1",
		modelId: "deterministic-summary-model-v1",
		endpointUrl: "https://summary.stub.invalid/v1/chat/completions",
		credentialRef: { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" },
	},
	readiness: "pending_privacy_boundary",
} as const;

type ObserverStatusRouteFactory = (deps: {
	getStore: () => unknown;
	getSweeper: () => null;
	getObserver: () => null;
	getCapabilitySnapshot: () => typeof frozenStatus;
}) => ReturnType<typeof observerStatusRoutes>;

describe("viewer frozen observer status", () => {
	it("reports one frozen safe identity without probing later legacy credentials", async () => {
		const previousOpenAI = process.env.OPENAI_API_KEY;
		const store = {
			rawEventBacklogTotals: () => ({ pending: 0, sessions: 0 }),
			latestRawEventFlushFailure: () => null,
		};
		const createRoutes = observerStatusRoutes as unknown as ObserverStatusRouteFactory;
		const routes = createRoutes({
			getStore: () => store,
			getSweeper: () => null,
			getObserver: () => null,
			getCapabilitySnapshot: () => frozenStatus,
		});

		try {
			const before = (await (await routes.request("/api/observer-status")).json()) as Record<
				string,
				unknown
			>;
			process.env.OPENAI_API_KEY = "fixture-late-legacy-token";
			const after = (await (await routes.request("/api/observer-status")).json()) as Record<
				string,
				unknown
			>;

			expect(before.capability).toEqual(frozenStatus);
			expect(before.active).toBeNull();
			expect(after).toEqual(before);
			expect(before).toHaveProperty("available_credentials", {});
			expect(JSON.stringify(before)).not.toContain("fixture-late-legacy-token");
		} finally {
			if (previousOpenAI === undefined) delete process.env.OPENAI_API_KEY;
			else process.env.OPENAI_API_KEY = previousOpenAI;
		}
	});
});

describe("observer status queue boundary", () => {
	it("counts only boundary-eligible pending events, and still counts them", async () => {
		const dir = mkdtempSync(join(tmpdir(), "codemem-observer-status-"));
		const store = openTestMemoryStore(join(dir, "memory.sqlite"));
		try {
			for (const fixture of [
				["eligible-session", "eligible"],
				["secret-session", "secret"],
			] as const) {
				const [streamId, sensitivity] = fixture;
				store.recordRawEvent({
					opencodeSessionId: streamId,
					eventId: `${streamId}-event`,
					eventType: "message",
					payload: { sentinel: `${streamId}-payload` },
					repositoryIdentity: null,
					sensitivity: sensitivity as SensitivityV1,
				});
			}
			const routes = observerStatusRoutes({
				getStore: () => store,
				getSweeper: () => null,
			});
			const body = (await (await routes.request("/api/observer-status")).json()) as {
				queue: { pending: number; sessions: number };
			};

			// Gate fires: the secret session never surfaces even as a count.
			// Gate passes: the eligible session is still counted.
			expect(body.queue.pending).toBe(1);
			expect(body.queue.sessions).toBe(1);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	it("shows only boundary-visible flush failures, projected onto closed codes", async () => {
		const dir = mkdtempSync(join(tmpdir(), "codemem-observer-failure-"));
		const store = openTestMemoryStore(join(dir, "memory.sqlite"));
		try {
			for (const fixture of [
				["eligible-stream", "eligible"],
				["secret-stream", "secret"],
			] as const) {
				const [streamId, sensitivity] = fixture;
				store.recordRawEvent({
					opencodeSessionId: streamId,
					eventId: `${streamId}-event`,
					eventType: "message",
					payload: { sentinel: `${streamId}-payload` },
					repositoryIdentity: null,
					sensitivity: sensitivity as SensitivityV1,
				});
			}
			const insertFailure = (streamId: string, updatedAt: string) =>
				store.db
					.prepare(
						`INSERT INTO raw_event_flush_batches(
							source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
							extractor_version, status, attempt_count, claim_generation,
							error_message, error_type, created_at, updated_at
						 ) VALUES ('opencode', ?, ?, 0, 1, 'raw_events_v1', 'failed', 2, 0,
							?, 'output_invalid', ?, ?)`,
					)
					.run(streamId, streamId, `${streamId}-error-message-sentinel`, updatedAt, updatedAt);
			// A mixed stream: eligible event first, secret event second. A batch
			// covering ONLY the secret range must stay hidden even though the
			// stream has a visible event elsewhere.
			for (const fixture of [
				["mixed-visible-event", "eligible"],
				["mixed-secret-event", "secret"],
			] as const) {
				const [eventId, sensitivity] = fixture;
				store.recordRawEvent({
					opencodeSessionId: "mixed-stream",
					eventId,
					eventType: "message",
					payload: { sentinel: `${eventId}-payload` },
					repositoryIdentity: null,
					sensitivity: sensitivity as SensitivityV1,
				});
			}
			const secretSeq = Number(
				store.db
					.prepare("SELECT event_seq FROM raw_events WHERE event_id = 'mixed-secret-event'")
					.pluck()
					.get(),
			);
			// The restricted failures are NEWER — an unfiltered latest-row lookup
			// would pick them. The mixed-stream batch also carries free-text
			// sentinels in its TEXT columns.
			insertFailure("eligible-stream", "2026-09-01T00:00:00.000Z");
			insertFailure("secret-stream", "2026-09-02T00:00:00.000Z");
			store.db
				.prepare(
					`INSERT INTO raw_event_flush_batches(
						source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
						extractor_version, status, attempt_count, claim_generation,
						error_message, error_type, created_at, updated_at
					 ) VALUES ('opencode', 'mixed-stream', 'mixed-stream', ?, ?, 'raw_events_v1', 'failed', 1, 0,
						'mixed-error-message-sentinel', 'PRIVATE-ERROR-TYPE-SENTINEL',
						'2026-09-03T00:00:00.000Z', '2026-09-03T00:00:00.000Z')`,
				)
				.run(secretSeq, secretSeq);

			const routes = observerStatusRoutes({
				getStore: () => store,
				getSweeper: () => null,
			});
			const body = (await (await routes.request("/api/observer-status")).json()) as {
				latest_failure: Record<string, unknown> | null;
			};

			// Gate passes: the eligible stream's failure is reported, its stored
			// arbitrary error_type mapped onto the closed vocabulary.
			expect(body.latest_failure).toMatchObject({
				status: "error",
				error_type: "unexpected_error",
				updated_at: "2026-09-01T00:00:00.000Z",
			});
			// Gate fires: no stream/session identifiers, no free-text messages,
			// nothing from the restricted stream, and nothing from a batch whose
			// event range is restricted even though its stream has visible events.
			const serialized = JSON.stringify(body);
			expect(serialized).not.toContain("secret-stream");
			expect(serialized).not.toContain("error-message-sentinel");
			expect(serialized).not.toContain("SENTINEL");
			expect(body.latest_failure).not.toHaveProperty("stream_id");
			expect(body.latest_failure).not.toHaveProperty("error_message");
			expect(body.latest_failure).not.toHaveProperty("observer_error_code");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
