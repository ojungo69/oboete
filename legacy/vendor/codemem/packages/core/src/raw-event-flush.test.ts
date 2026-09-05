import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { compileDefaultCapabilityManifest } from "./capability-manifest.js";
import { buildRawEventEnvelopeFromHook } from "./claude-hooks.js";
import { connect } from "./db.js";
import type { IngestOptions } from "./ingest-pipeline.js";
import { ObserverAuthError } from "./observer-client.js";
import { flushRawEvents } from "./raw-event-flush.js";
import type { MemoryStore } from "./store.js";
import { initTestSchema, openTestMemoryStore } from "./test-utils.js";

describe("flushRawEvents max retry", () => {
	let tmpDir: string;
	let store: MemoryStore;
	let prevMaxAttempts: string | undefined;

	beforeEach(() => {
		prevMaxAttempts = process.env.CODEMEM_RAW_EVENTS_MAX_FLUSH_ATTEMPTS;
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-flush-test-"));
		const dbPath = join(tmpDir, "test.sqlite");
		const db = connect(dbPath);
		initTestSchema(db);
		db.close();
		store = openTestMemoryStore(dbPath);
	});

	afterEach(() => {
		store.close();
		if (prevMaxAttempts === undefined) delete process.env.CODEMEM_RAW_EVENTS_MAX_FLUSH_ATTEMPTS;
		else process.env.CODEMEM_RAW_EVENTS_MAX_FLUSH_ATTEMPTS = prevMaxAttempts;
		rmSync(tmpDir, { recursive: true, force: true });
	});

	function seedEvents(sessionId: string) {
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-1",
			eventType: "user_prompt",
			payload: { type: "user_prompt", prompt_text: "Hello" },
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: 100,
		});
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-2",
			eventType: "tool.execute.after",
			payload: { type: "tool.execute.after", tool: "read", args: { filePath: "/tmp/x.ts" } },
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: 200,
		});
	}

	const nullObserver = {
		observe: async () => ({
			raw: null as string | null,
			parsed: null,
			provider: "test",
			model: "test-model",
		}),
		getStatus: () => ({
			provider: "test",
			model: "test-model",
			runtime: "test",
			auth: { source: "none", type: "none", hasToken: false },
		}),
	};

	const summaryObserver = {
		observe: async () => ({
			raw: `<summary>
				<request>Investigate auth timeout</request>
				<investigated>Session handling code</investigated>
				<learned>Race condition in handler</learned>
				<completed>Added callback validation</completed>
				<next_steps>Add regression test</next_steps>
				<notes></notes>
				<citations><cite source="0"/></citations>
			</summary>`,
			parsed: null,
			provider: "test",
			model: "test-model",
		}),
		getStatus: () => ({
			provider: "test",
			model: "test-model",
			runtime: "test",
			auth: { source: "none", type: "none", hasToken: false },
		}),
	};
	const repositoryIdentity = `repo-v1:sha256:${"c".repeat(64)}`;
	const manifest = (endpointUrl: string) =>
		compileDefaultCapabilityManifest({
			version: 1,
			role: "summary",
			state: "enabled",
			wireProtocol: "openai_chat_completions_v1",
			modelId: "raw-event-flush-test",
			modelRevision: "1",
			endpointUrl,
			credentialRef: { kind: "none" },
		});
	const remoteManifest = manifest("https://summary.stub.invalid/v1/chat/completions");
	const localHttpManifest = manifest("http://127.0.0.1:1234/v1/chat/completions");
	const localHttpsManifest = manifest("https://127.0.0.1:1234/v1/chat/completions");
	const ingestOptions = (
		observer: unknown,
		capabilityManifest = remoteManifest,
		providerTlsPeerVerified = true,
	): IngestOptions =>
		({
			observer,
			capabilityManifest,
			resourceProfile: capabilityManifest.resourceProfile,
			configurationFingerprint: capabilityManifest.configurationFingerprint,
			providerFingerprint: capabilityManifest.summaryProvider.providerFingerprint,
			providerTlsPeerVerified,
		}) as IngestOptions;
	const recordPrompt = (input: {
		sessionId: string;
		eventId: string;
		prompt: string;
		sensitivity: "eligible" | "private" | "local_only" | "secret";
		repositoryIdentity: string | null;
		tsWallMs: number;
	}) =>
		store.recordRawEvent({
			opencodeSessionId: input.sessionId,
			eventId: input.eventId,
			eventType: "user_prompt",
			payload: { type: "user_prompt", prompt_text: input.prompt },
			repositoryIdentity: input.repositoryIdentity,
			sensitivity: input.sensitivity,
			tsWallMs: input.tsWallMs,
		});

	it("exhausts after three automatic attempts without advancing the flush cursor", async () => {
		const sessionId = "ses_max_retry_test";
		seedEvents(sessionId);

		const ingestOpts = ingestOptions(nullObserver);
		const flushOpts = {
			opencodeSessionId: sessionId,
			source: "opencode",
			cwd: null,
			project: null,
			startedAt: null,
		};

		for (let i = 0; i < 3; i++) {
			await expect(flushRawEvents(store, ingestOpts, flushOpts)).rejects.toThrow(
				"observer failed during raw-event flush",
			);
		}

		// Timer passage never resumes retry-exhausted work.
		const result = await flushRawEvents(store, ingestOpts, flushOpts);
		expect(result).toEqual({ flushed: 0, updatedState: 0 });

		const batch = store.db
			.prepare(
				"SELECT status, attempt_count FROM raw_event_flush_batches WHERE opencode_session_id = ?",
			)
			.get(sessionId) as { status: string; attempt_count: number };
		expect(batch.status).toBe("retry_exhausted");
		expect(batch.attempt_count).toBe(3);
		expect(store.latestRawEventFlushFailure("opencode")).toMatchObject({
			stream_id: sessionId,
			status: "error",
		});

		expect(store.rawEventFlushState(sessionId, "opencode")).toBe(-1);
		expect(
			store.db.prepare("SELECT ended_at FROM sessions ORDER BY id DESC LIMIT 1").get(),
		).toEqual({ ended_at: null });
	});

	it("completes an accepted trivial prompt without retrying or consuming capacity", async () => {
		const sessionId = "ses_trivial_prompt";
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-yes",
			eventType: "user_prompt",
			payload: { type: "user_prompt", prompt_text: "yes" },
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: 100,
		});
		let observerCalls = 0;
		const observer = {
			observe: async () => {
				observerCalls++;
				throw new Error("observer must not be called");
			},
			getStatus: nullObserver.getStatus,
		};

		await expect(
			flushRawEvents(store, ingestOptions(observer), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).resolves.toEqual({ flushed: 1, updatedState: 1 });

		expect(observerCalls).toBe(0);
		expect(store.rawEventFlushState(sessionId, "opencode")).toBe(0);
		expect(
			store.db
				.prepare(
					`SELECT status, attempt_count, completion_disposition, output_count,
						observed_output_count, safe_error_code
					 FROM raw_event_flush_batches WHERE stream_id = ?`,
				)
				.get(sessionId),
		).toEqual({
			status: "completed",
			attempt_count: 1,
			completion_disposition: "memory_committed",
			output_count: 0,
			observed_output_count: 0,
			safe_error_code: null,
		});
		expect(
			store.db
				.prepare(
					"SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE status IN ('queued','processing','failed','retry_exhausted')",
				)
				.get(),
		).toEqual({ count: 0 });
		expect(store.latestRawEventFlushFailure("opencode")).toBeNull();
	});

	it("completes long lifecycle-only ranges without calling the observer", async () => {
		for (const [source, sessionId] of [
			["opencode", "ses_lifecycle_top_level"],
			["claude", "ses_lifecycle_adapter"],
		] as const) {
			for (let index = 0; index < 5; index++) {
				if (source === "opencode") {
					const type = ["session.started", "session.idle", "session.ended"][index % 3] ?? "";
					store.recordRawEvent({
						opencodeSessionId: sessionId,
						source,
						eventId: `${sessionId}-${index}`,
						eventType: type,
						payload: { type },
						repositoryIdentity,
						sensitivity: "eligible",
						tsWallMs: index * 1_000,
					});
					continue;
				}
				const envelope = buildRawEventEnvelopeFromHook({
					hook_event_name: index % 2 === 0 ? "SessionStart" : "SessionEnd",
					session_id: sessionId,
					source: `startup-${index}`,
					reason: `reason-${index}`,
					ts: new Date(index * 1_000).toISOString(),
				});
				if (!envelope) throw new Error("expected lifecycle adapter envelope");
				store.recordRawEvent({
					opencodeSessionId: envelope.opencode_session_id,
					source: envelope.source,
					eventId: envelope.event_id,
					eventType: envelope.event_type,
					payload: envelope.payload,
					repositoryIdentity,
					sensitivity: "eligible",
					tsWallMs: envelope.ts_wall_ms,
				});
			}
			let observerCalls = 0;
			const observer = {
				observe: async () => {
					observerCalls++;
					throw new Error("observer must not be called");
				},
				getStatus: nullObserver.getStatus,
			};

			await expect(
				flushRawEvents(store, ingestOptions(observer), {
					opencodeSessionId: sessionId,
					source,
					cwd: null,
					project: null,
					startedAt: null,
				}),
			).resolves.toEqual({ flushed: 5, updatedState: 1 });
			expect(observerCalls).toBe(0);
			expect(store.rawEventFlushState(sessionId, source)).toBe(4);
			expect(
				store.db
					.prepare(
						`SELECT status, attempt_count, completion_disposition, egress_diagnostic_json
						 FROM raw_event_flush_batches WHERE source = ? AND stream_id = ?`,
					)
					.get(source, sessionId),
			).toMatchObject({
				status: "completed",
				attempt_count: 1,
				completion_disposition: "memory_committed",
				egress_diagnostic_json: expect.stringContaining('"reason":"eligible_only"'),
			});
		}
		expect(
			store.db
				.prepare(
					"SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE status IN ('queued','processing','failed','retry_exhausted')",
				)
				.get(),
		).toEqual({ count: 0 });
	});

	it("keeps adapter errors retryable instead of terminal low-signal", async () => {
		const sessionId = "ses_adapter_error";
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			source: "claude",
			eventId: "adapter-error-0",
			eventType: "claude.hook",
			payload: {
				type: "claude.hook",
				_adapter: {
					schema_version: "1.0",
					source: "claude",
					session_id: sessionId,
					event_id: "adapter-error-0",
					event_type: "error",
					payload: { message: "adapter failed" },
					ts: "2026-03-04T10:00:00.000Z",
				},
			},
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: 100,
		});

		await expect(
			flushRawEvents(store, ingestOptions(nullObserver), {
				opencodeSessionId: sessionId,
				source: "claude",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toThrow("observer produced no storable output for raw-event flush");
		expect(
			store.db
				.prepare(
					`SELECT status, attempt_count, completion_disposition, safe_error_code
					 FROM raw_event_flush_batches WHERE source = 'claude' AND stream_id = ?`,
				)
				.get(sessionId),
		).toEqual({
			status: "failed",
			attempt_count: 1,
			completion_disposition: "none",
			safe_error_code: "output_invalid",
		});
	});

	it("keeps a substantive prompt after a trivial prompt retryable", async () => {
		const sessionId = "ses_mixed_prompts";
		for (const [eventId, promptText, tsWallMs] of [
			["evt-yes", "yes", 100],
			["evt-substantive", "Investigate the durable retry path", 200],
		] as const) {
			store.recordRawEvent({
				opencodeSessionId: sessionId,
				eventId,
				eventType: "user_prompt",
				payload: { type: "user_prompt", prompt_text: promptText },
				repositoryIdentity,
				sensitivity: "eligible",
				tsWallMs,
			});
		}
		await expect(
			flushRawEvents(store, ingestOptions(nullObserver), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toThrow("observer produced no storable output for raw-event flush");
		expect(store.rawEventFlushState(sessionId, "opencode")).toBe(-1);
		expect(
			store.db
				.prepare("SELECT status, attempt_count FROM raw_event_flush_batches WHERE stream_id = ?")
				.get(sessionId),
		).toEqual({ status: "failed", attempt_count: 1 });
	});

	it("keeps an unknown accepted event retryable", async () => {
		const sessionId = "ses_unknown_event";
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-unknown",
			eventType: "unknown.event",
			payload: { type: "unknown.event" },
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: 100,
		});

		await expect(
			flushRawEvents(store, ingestOptions(nullObserver), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toThrow("observer produced no storable output for raw-event flush");

		expect(store.rawEventFlushState(sessionId, "opencode")).toBe(-1);
		expect(
			store.db
				.prepare(
					`SELECT batch.status, batch.attempt_count, COUNT(event.id) AS source_count
					 FROM raw_event_flush_batches AS batch
					 JOIN raw_events AS event ON event.source = batch.source
						AND event.stream_id = batch.stream_id
						AND event.event_seq BETWEEN batch.start_event_seq AND batch.end_event_seq
					 WHERE batch.stream_id = ?`,
				)
				.get(sessionId),
		).toEqual({ status: "failed", attempt_count: 1, source_count: 1 });
	});

	it("records the closed diagnostic for observer output above the active limit", async () => {
		const sessionId = "ses_output_limit";
		seedEvents(sessionId);
		const observer = {
			observe: async () => ({
				raw: Array.from(
					{ length: 17 },
					(_, index) =>
						`<observation><type>discovery</type><title>Result ${index}</title><narrative>Distinct durable result ${index}.</narrative><citations><cite source="0" start="${index}" end="${index + 1}"/></citations></observation>`,
				).join(""),
				parsed: null,
				provider: "test",
				model: "test-model",
			}),
			getStatus: summaryObserver.getStatus,
		};

		await expect(
			flushRawEvents(store, ingestOptions(observer), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toThrow("output limit exceeded");
		expect(
			store.db
				.prepare("SELECT status, safe_error_code FROM raw_event_flush_batches WHERE stream_id = ?")
				.get(sessionId),
		).toEqual({ status: "failed", safe_error_code: "output_limit_exceeded" });
		expect(
			store.db.prepare("SELECT ended_at FROM sessions ORDER BY id DESC LIMIT 1").get(),
		).toEqual({ ended_at: null });
	});

	it("preserves the observer failure when legacy diagnostic persistence also fails", async () => {
		const sessionId = "ses_diagnostic_failure";
		seedEvents(sessionId);
		vi.spyOn(store, "recordRawEventFlushBatchDiagnostic").mockImplementation(() => {
			throw new Error("legacy diagnostic failure");
		});

		await expect(
			flushRawEvents(store, ingestOptions(nullObserver), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toThrow("observer failed during raw-event flush");
		expect(
			store.db
				.prepare("SELECT status FROM raw_event_flush_batches WHERE opencode_session_id = ?")
				.get(sessionId),
		).toEqual({ status: "failed" });
	});

	it("preserves an auth failure when its processing claim became stale", async () => {
		const sessionId = "ses_stale_auth_failure";
		seedEvents(sessionId);
		const authError = new ObserverAuthError("auth failed");
		const observer = {
			observe: async () => {
				store.db
					.prepare(
						"UPDATE raw_event_flush_batches SET claim_generation = claim_generation + 1 WHERE stream_id = ? AND status = 'processing'",
					)
					.run(sessionId);
				throw authError;
			},
			getStatus: nullObserver.getStatus,
		};

		await expect(
			flushRawEvents(store, ingestOptions(observer), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toBe(authError);
	});

	it("surfaces a canonical job failure persistence error", async () => {
		const sessionId = "ses_failure_persistence";
		seedEvents(sessionId);
		const persistenceError = new Error("job failure persistence failed");
		vi.spyOn(store, "failRawEventFlushJob").mockImplementation(() => {
			throw persistenceError;
		});

		await expect(
			flushRawEvents(store, ingestOptions(nullObserver), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toBe(persistenceError);
	});

	it("privacy-skips an all-quarantined range without calling the observer", async () => {
		const sessionId = "ses_all_quarantined";
		expect(
			store.recordRawEvent({
				opencodeSessionId: sessionId,
				eventId: "quarantined-0",
				eventType: "user_prompt",
				payload: { text: "must not reach provider" },
				sensitivity: "secret",
				captureState: "quarantined",
				safeErrorCode: "redaction_degraded",
				redactionDegraded: true,
			}),
		).toMatchObject({ status: "quarantined", normalAck: false });
		let observerCalls = 0;
		const observer = {
			observe: async () => {
				observerCalls++;
				throw new Error("observer must not be called");
			},
			getStatus: nullObserver.getStatus,
		};
		expect(
			await flushRawEvents(store, ingestOptions(observer), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).toEqual({ flushed: 1, updatedState: 1 });
		expect(observerCalls).toBe(0);
		expect(store.rawEventFlushState(sessionId, "opencode")).toBe(0);
		expect(
			store.db
				.prepare(
					"SELECT payload_json, sensitivity, capture_state, safe_error_code FROM raw_events WHERE event_id = 'quarantined-0'",
				)
				.get(),
		).toEqual({
			payload_json: "{}",
			sensitivity: "secret",
			capture_state: "quarantined",
			safe_error_code: "redaction_degraded",
		});
		expect(
			store.db
				.prepare(
					"SELECT status, completion_disposition FROM raw_event_flush_batches WHERE stream_id = ?",
				)
				.get(sessionId),
		).toEqual({ status: "completed", completion_disposition: "privacy_skip" });
	});

	it("omits quarantined positions from mixed provider input and memory citations", async () => {
		const sessionId = "ses_mixed_quarantine";
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "mixed-accepted",
			eventType: "user_prompt",
			payload: { type: "user_prompt", prompt_text: "VISIBLE_MIXED_EVENT" },
			repositoryIdentity,
			sensitivity: "eligible",
		});
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "mixed-quarantined",
			eventType: "user_prompt",
			payload: { type: "user_prompt", prompt_text: "FORBIDDEN_MIXED_EVENT" },
			repositoryIdentity,
			sensitivity: "secret",
			captureState: "quarantined",
			safeErrorCode: "redaction_degraded",
			redactionDegraded: true,
		});
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "mixed-accepted-assistant",
			eventType: "assistant_message",
			payload: { type: "assistant_message", assistant_text: "Visible completion" },
			repositoryIdentity,
			sensitivity: "eligible",
		});
		let requestText = "";
		const observer = {
			observe: async (system: string, user: string) => {
				requestText = `${system}\n${user}`;
				return summaryObserver.observe();
			},
			getStatus: summaryObserver.getStatus,
		};
		expect(
			await flushRawEvents(store, ingestOptions(observer), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).toEqual({ flushed: 3, updatedState: 1 });
		expect(requestText).toContain("VISIBLE_MIXED_EVENT");
		expect(requestText).not.toContain("FORBIDDEN_MIXED_EVENT");
		expect(
			store.db
				.prepare(
					"SELECT source_event_ids_json FROM memory_items WHERE source_event_ids_json IS NOT NULL ORDER BY id DESC LIMIT 1",
				)
				.get(),
		).toEqual({
			source_event_ids_json: '["mixed-accepted"]',
		});
	});

	it("projects only eligible events to a remote provider in source order", async () => {
		const sessionId = "ses_remote_projection";
		for (const [eventId, prompt, sensitivity, tsWallMs] of [
			["remote-eligible-0", "VISIBLE_REMOTE_FIRST", "eligible", 100],
			["remote-private", "FORBIDDEN_REMOTE_PRIVATE", "private", 200],
			["remote-local-only", "FORBIDDEN_REMOTE_LOCAL_ONLY", "local_only", 300],
			["remote-secret", "FORBIDDEN_REMOTE_SECRET", "secret", 400],
			["remote-eligible-1", "VISIBLE_REMOTE_SECOND", "eligible", 500],
		] as const) {
			recordPrompt({
				sessionId,
				eventId,
				prompt,
				sensitivity,
				repositoryIdentity,
				tsWallMs,
			});
		}
		let observerInput = "";
		const observer = {
			observe: async (system: string, user: string) => {
				observerInput = `${system}\n${user}`;
				return {
					raw: `<observation>
						<type>discovery</type><title>Retain eligible remote events</title>
						<narrative>Restricted events never left the host.
						The projected remote source set retained eligible work.
						Stored the eligible result for later sessions.
						</narrative>
						<citations><cite source="0"/><cite source="1"/></citations>
					</observation>`,
					parsed: null,
					provider: "test",
					model: "test-model",
				};
			},
			getStatus: summaryObserver.getStatus,
		};

		expect(
			await flushRawEvents(store, ingestOptions(observer, remoteManifest), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).toEqual({ flushed: 5, updatedState: 1 });
		expect(observerInput).not.toContain("FORBIDDEN_REMOTE_PRIVATE");
		expect(observerInput).not.toContain("FORBIDDEN_REMOTE_LOCAL_ONLY");
		expect(observerInput).not.toContain("FORBIDDEN_REMOTE_SECRET");
		expect(observerInput.indexOf("VISIBLE_REMOTE_FIRST")).toBeGreaterThanOrEqual(0);
		expect(observerInput.indexOf("VISIBLE_REMOTE_SECOND")).toBeGreaterThan(
			observerInput.indexOf("VISIBLE_REMOTE_FIRST"),
		);
	});

	it.each([
		[
			"an all-restricted remote range",
			remoteManifest,
			true,
			[
				["private", repositoryIdentity, "FORBIDDEN_REMOTE_PRIVATE"],
				["local_only", repositoryIdentity, "FORBIDDEN_REMOTE_LOCAL_ONLY"],
				["secret", repositoryIdentity, "FORBIDDEN_REMOTE_SECRET"],
			],
		],
		[
			"an all-restricted unverified local HTTP range",
			localHttpManifest,
			false,
			[
				["private", repositoryIdentity, "FORBIDDEN_HTTP_PRIVATE"],
				["local_only", repositoryIdentity, "FORBIDDEN_HTTP_LOCAL_ONLY"],
				["secret", repositoryIdentity, "FORBIDDEN_HTTP_SECRET"],
			],
		],
		[
			"an unknown-repository verified local HTTPS range",
			localHttpsManifest,
			true,
			[["private", null, "FORBIDDEN_UNKNOWN_REPO"]],
		],
	] as const)("atomically privacy-skips an all-restricted range for %s", async (_label, capabilityManifest, providerTlsPeerVerified, events) => {
		const sessionId = "ses_destination_privacy_skip";
		for (const [index, [sensitivity, eventRepositoryIdentity, prompt]] of events.entries()) {
			recordPrompt({
				sessionId,
				eventId: `restricted-${index}`,
				prompt,
				sensitivity,
				repositoryIdentity: eventRepositoryIdentity,
				tsWallMs: index + 1,
			});
		}
		let observerCalls = 0;
		const observer = {
			observe: async () => {
				observerCalls++;
				throw new Error("observer must not be called");
			},
			getStatus: nullObserver.getStatus,
		};

		expect(
			await flushRawEvents(
				store,
				ingestOptions(observer, capabilityManifest, providerTlsPeerVerified),
				{
					opencodeSessionId: sessionId,
					source: "opencode",
					cwd: null,
					project: null,
					startedAt: null,
				},
			),
		).toEqual({ flushed: events.length, updatedState: 1 });
		expect(observerCalls).toBe(0);
		expect(store.recent(10)).toHaveLength(0);
		const batch = store.db
			.prepare(
				"SELECT status, completion_disposition, egress_diagnostic_json FROM raw_event_flush_batches WHERE stream_id = ?",
			)
			.get(sessionId) as {
			status: string;
			completion_disposition: string;
			egress_diagnostic_json: string;
		};
		expect(batch).toMatchObject({
			status: "completed",
			completion_disposition: "privacy_skip",
		});
		for (const [, , prompt] of events) {
			expect(batch.egress_diagnostic_json).not.toContain(prompt);
		}
	});

	it("allows eligible, private, and local-only events only for verified local HTTPS in the same repository", async () => {
		const sessionId = "ses_verified_local_projection";
		for (const [eventId, prompt, sensitivity, tsWallMs] of [
			["local-eligible", "VISIBLE_LOCAL_ELIGIBLE", "eligible", 100],
			["local-private", "VISIBLE_LOCAL_PRIVATE", "private", 200],
			["local-only", "VISIBLE_LOCAL_ONLY", "local_only", 300],
			["local-secret", "FORBIDDEN_LOCAL_SECRET", "secret", 400],
		] as const) {
			recordPrompt({
				sessionId,
				eventId,
				prompt,
				sensitivity,
				repositoryIdentity,
				tsWallMs,
			});
		}
		let observerInput = "";
		const observer = {
			observe: async (system: string, user: string) => {
				observerInput = `${system}\n${user}`;
				return {
					raw: `<observation>
						<type>discovery</type><title>Retain same-repository local events</title>
						<narrative>The verified local projection excluded only the secret source.
						Stored the local result for later sessions.
						</narrative>
						<citations><cite source="0"/><cite source="1"/><cite source="2"/></citations>
					</observation>`,
					parsed: null,
					provider: "test",
					model: "test-model",
				};
			},
			getStatus: summaryObserver.getStatus,
		};

		expect(
			await flushRawEvents(store, ingestOptions(observer, localHttpsManifest, true), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).toEqual({ flushed: 4, updatedState: 1 });
		const eligibleIndex = observerInput.indexOf("VISIBLE_LOCAL_ELIGIBLE");
		const privateIndex = observerInput.indexOf("VISIBLE_LOCAL_PRIVATE");
		const localOnlyIndex = observerInput.indexOf("VISIBLE_LOCAL_ONLY");
		expect(eligibleIndex).toBeGreaterThanOrEqual(0);
		expect(privateIndex).toBeGreaterThan(eligibleIndex);
		expect(localOnlyIndex).toBeGreaterThan(privateIndex);
		expect(observerInput).not.toContain("FORBIDDEN_LOCAL_SECRET");
	});

	it("does not give up when under the max attempts", async () => {
		const sessionId = "ses_under_max";
		seedEvents(sessionId);

		const ingestOpts = ingestOptions(nullObserver);
		const flushOpts = {
			opencodeSessionId: sessionId,
			source: "opencode",
			cwd: null,
			project: null,
			startedAt: null,
		};

		// Fail twice — still under the limit
		for (let i = 0; i < 2; i++) {
			await expect(flushRawEvents(store, ingestOpts, flushOpts)).rejects.toThrow(
				"observer failed during raw-event flush",
			);
		}

		const batch = store.db
			.prepare("SELECT status FROM raw_event_flush_batches WHERE opencode_session_id = ?")
			.get(sessionId) as { status: string };
		expect(batch.status).toBe("failed");
	});

	it("keeps a lossy batch retryable when the repair is rejected", async () => {
		const sessionId = "ses_rejected_lossy_repair";
		seedEvents(sessionId);
		let callCount = 0;
		const lossy = `<observation>
			<type>discovery</type><title>Retained result</title>
			<narrative>Keep this valid observation.</narrative>
			<citations><cite source="0"/></citations>
		</observation><observation><type>bugfix</type><title>Truncated result`;
		const unrelatedRepair = `<observation>
			<type>discovery</type><title>Unrelated result</title>
			<narrative>This does not recover the truncated block.</narrative>
			<citations><cite source="0"/></citations>
		</observation>`;
		const observer = {
			observe: async () => {
				callCount += 1;
				return {
					raw: callCount === 1 ? lossy : unrelatedRepair,
					parsed: null,
					provider: "test",
					model: "test-model",
				};
			},
			getStatus: () => ({
				provider: "test",
				model: "test-model",
				runtime: "test",
				auth: { source: "none", type: "none", hasToken: false },
			}),
		};

		await expect(
			flushRawEvents(store, ingestOptions(observer), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toThrow("observer repair remained lossy during raw-event flush");

		expect(callCount).toBe(2);
		expect(store.rawEventFlushState(sessionId, "opencode")).toBe(-1);
		const batch = store.db
			.prepare(
				"SELECT status, error_message FROM raw_event_flush_batches WHERE opencode_session_id = ?",
			)
			.get(sessionId) as { status: string; error_message: string };
		expect(batch.status).toBe("failed");
		expect(batch.error_message).toBe(
			"Test returned structurally incomplete output that could not be repaired.",
		);
		expect(store.recent(10)).toHaveLength(0);
	});

	it("keeps a lossy skip fallback retryable when repair cannot recover it", async () => {
		const sessionId = "ses_lossy_skip_repair";
		seedEvents(sessionId);
		let callCount = 0;
		const observer = {
			observe: async () => {
				callCount += 1;
				return {
					raw:
						callCount === 1
							? `<skip_summary reason="low-signal"/><observation><type>bugfix</type><title>Truncated durable result`
							: `<summary><request>Unrelated repair</request><citations><cite source="0"/></citations></summary>`,
					parsed: null,
					provider: "test",
					model: "test-model",
				};
			},
			getStatus: () => ({
				provider: "test",
				model: "test-model",
				runtime: "test",
				auth: { source: "none", type: "none", hasToken: false },
			}),
		};

		await expect(
			flushRawEvents(store, ingestOptions(observer), {
				opencodeSessionId: sessionId,
				source: "opencode",
				cwd: null,
				project: null,
				startedAt: null,
			}),
		).rejects.toThrow("observer repair remained lossy during raw-event flush");

		expect(callCount).toBe(2);
		expect(store.rawEventFlushState(sessionId, "opencode")).toBe(-1);
		const batch = store.db
			.prepare("SELECT status FROM raw_event_flush_batches WHERE opencode_session_id = ?")
			.get(sessionId) as { status: string };
		expect(batch.status).toBe("failed");
		expect(store.recent(10)).toHaveLength(0);
	});

	it("retry-exhausted batches are not resurrected by broad retry maintenance", async () => {
		const sessionId = "ses_no_resurrect";
		seedEvents(sessionId);

		const ingestOpts = ingestOptions(nullObserver);
		const flushOpts = {
			opencodeSessionId: sessionId,
			source: "opencode",
			cwd: null,
			project: null,
			startedAt: null,
		};

		for (let attempt = 0; attempt < 3; attempt++) {
			await expect(flushRawEvents(store, ingestOpts, flushOpts)).rejects.toThrow();
		}

		const before = store.db
			.prepare("SELECT status FROM raw_event_flush_batches WHERE opencode_session_id = ?")
			.get(sessionId) as { status: string };
		expect(before.status).toBe("retry_exhausted");

		const { retryRawEventFailuresWithDb } = await import("./maintenance.js");
		const retried = retryRawEventFailuresWithDb(store.db);
		expect(retried.retried).toBe(0);

		const after = store.db
			.prepare("SELECT status FROM raw_event_flush_batches WHERE opencode_session_id = ?")
			.get(sessionId) as { status: string };
		expect(after.status).toBe("retry_exhausted");
	});

	it("keeps direct flush admission contiguous and capped at 100 source events", async () => {
		const sessionId = "ses-slice1-max-100";
		for (let eventSeq = 0; eventSeq < 101; eventSeq++) {
			store.recordRawEvent({
				opencodeSessionId: sessionId,
				eventId: `evt-${eventSeq}`,
				eventType: "user_prompt",
				payload: { type: "user_prompt", prompt_text: `event ${eventSeq}` },
				repositoryIdentity,
				sensitivity: "eligible",
				tsWallMs: eventSeq,
			});
		}
		const opts = {
			opencodeSessionId: sessionId,
			source: "opencode",
			cwd: null,
			project: null,
			startedAt: null,
		};
		const cappedObserver = {
			observe: async () => ({
				raw: '<skip_summary reason="low-signal"/>',
				parsed: null,
				provider: "test",
				model: "test-model",
			}),
			getStatus: summaryObserver.getStatus,
		};
		const cappedIngestOptions = ingestOptions(cappedObserver);
		expect(await flushRawEvents(store, cappedIngestOptions, opts)).toEqual({
			flushed: 100,
			updatedState: 1,
		});
		expect(store.rawEventFlushState(sessionId)).toBe(99);
		expect(await flushRawEvents(store, cappedIngestOptions, opts)).toEqual({
			flushed: 1,
			updatedState: 1,
		});
		expect(store.rawEventFlushState(sessionId)).toBe(100);
	});

	it("reuses one local session per stable raw-event session id", async () => {
		const sessionId = "ses_bridge_reuse";
		seedEvents(sessionId);

		const ingestOpts = ingestOptions(summaryObserver);
		const flushOpts = {
			opencodeSessionId: sessionId,
			source: "opencode",
			cwd: null,
			project: null,
			startedAt: "2026-03-01T10:00:00Z",
		};

		const first = await flushRawEvents(store, ingestOpts, flushOpts);
		expect(first.updatedState).toBe(1);

		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-3",
			eventType: "assistant_message",
			payload: { type: "assistant_message", assistant_text: "Added validation." },
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: 300,
		});
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-4",
			eventType: "tool.execute.after",
			payload: {
				type: "tool.execute.after",
				tool: "edit",
				args: { filePath: "/tmp/y.ts" },
			},
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: 400,
		});

		const second = await flushRawEvents(store, ingestOpts, flushOpts);
		expect(second.updatedState).toBe(1);

		const opencodeBridge = store.db
			.prepare(
				"SELECT session_id FROM opencode_sessions WHERE source = 'opencode' AND stream_id = ?",
			)
			.get(sessionId) as { session_id: number } | undefined;
		expect(opencodeBridge?.session_id).toBeDefined();

		const localSessionCount = store.db.prepare("SELECT COUNT(*) AS count FROM sessions").get() as {
			count: number;
		};
		expect(localSessionCount.count).toBe(1);

		const memorySessionCounts = store.db
			.prepare(
				"SELECT COUNT(DISTINCT session_id) AS count FROM memory_items WHERE active = 1 AND json_extract(metadata_json, '$.source') = 'observer_summary'",
			)
			.get() as { count: number };
		expect(memorySessionCounts.count).toBe(1);
	});

	it("populates session context fields from Claude Code adapter-enveloped raw events", async () => {
		const sessionId = "sess-claude-ctx";

		// Seed Claude Code hook events via the same envelope path the viewer/CLI
		// use. These produce `claude.hook` raw events with an `_adapter` payload.
		const hookEvents: Record<string, unknown>[] = [
			{
				hook_event_name: "UserPromptSubmit",
				session_id: sessionId,
				prompt: "Investigate the flush bug",
				cwd: "/tmp/repo",
				ts: "2026-03-04T10:00:00Z",
			},
			{
				hook_event_name: "PostToolUse",
				session_id: sessionId,
				tool_use_id: "toolu_1",
				tool_name: "Read",
				tool_input: { file_path: "/tmp/repo/src/flush.ts" },
				tool_response: "file contents",
				cwd: "/tmp/repo",
				ts: "2026-03-04T10:00:05Z",
			},
			{
				hook_event_name: "PostToolUse",
				session_id: sessionId,
				tool_use_id: "toolu_2",
				tool_name: "Edit",
				tool_input: { file_path: "/tmp/repo/src/flush.ts" },
				tool_response: "edited",
				cwd: "/tmp/repo",
				ts: "2026-03-04T10:00:10Z",
			},
		];

		for (const hook of hookEvents) {
			const envelope = buildRawEventEnvelopeFromHook(hook);
			expect(envelope).not.toBeNull();
			if (envelope == null) throw new Error("envelope");
			store.recordRawEvent({
				opencodeSessionId: envelope.opencode_session_id,
				source: envelope.source,
				eventId: envelope.event_id,
				eventType: envelope.event_type,
				payload: envelope.payload,
				repositoryIdentity,
				sensitivity: "eligible",
				tsWallMs: envelope.ts_wall_ms,
			});
		}

		const summaryResponder = {
			observe: async () => ({
				raw: `<summary>
					<request>Investigate the flush bug</request>
					<investigated>raw-event-flush.ts session context builder</investigated>
					<learned>Claude Code events need normalization before scanning</learned>
					<completed>Added normalization step</completed>
					<next_steps>Add regression test</next_steps>
					<notes></notes>
					<citations><cite source="0"/></citations>
				</summary>`,
				parsed: null,
				provider: "test",
				model: "test-model",
			}),
			getStatus: () => ({
				provider: "test",
				model: "test-model",
				runtime: "test",
				auth: { source: "none", type: "none", hasToken: false },
			}),
		};

		const ingestOpts = ingestOptions(summaryResponder);
		const flushOpts = {
			opencodeSessionId: sessionId,
			source: "claude",
			cwd: "/tmp/repo",
			project: "repo",
			startedAt: "2026-03-04T10:00:00Z",
		};

		const result = await flushRawEvents(store, ingestOpts, flushOpts);
		expect(result.flushed).toBeGreaterThan(0);
		expect(result.updatedState).toBe(1);

		// The persisted session metadata is the authoritative regression check:
		// before the fix these fields were all absent because Claude Code raw
		// events (type="claude.hook") never populated buildSessionContext's
		// counts or path lists.
		const row = store.db
			.prepare(
				"SELECT metadata_json FROM sessions WHERE id = (SELECT session_id FROM opencode_sessions WHERE stream_id = ?)",
			)
			.get(sessionId) as { metadata_json: string } | undefined;
		expect(row).toBeDefined();
		const meta = JSON.parse(row?.metadata_json ?? "{}") as {
			session_context?: {
				promptCount?: number;
				toolCount?: number;
				firstPrompt?: string;
				filesRead?: string[];
				filesModified?: string[];
			};
		};
		expect(meta.session_context?.promptCount).toBe(1);
		expect(meta.session_context?.toolCount).toBe(2);
		expect(meta.session_context?.firstPrompt).toBe("Investigate the flush bug");
		expect(meta.session_context?.filesRead).toEqual(["/tmp/repo/src/flush.ts"]);
		expect(meta.session_context?.filesModified).toEqual(["/tmp/repo/src/flush.ts"]);
	});

	it("populates filesModified from OpenCode apply_patch adapter events end-to-end", async () => {
		const sessionId = "ses_opencode_apply_patch";
		const patchText = [
			"*** Begin Patch",
			"*** Add File: /repo/src/new.ts",
			"+export const created = 1;",
			"*** Update File: /repo/src/existing.ts",
			"@@ -1 +1 @@",
			"-export const value = 1;",
			"+export const value = 2;",
			"*** End Patch",
		].join("\n");

		// Seed a PURE adapter-enveloped tool_result event — this is the shape
		// OpenCode actually emits for apply_patch. No outer `tool` / `args` /
		// `type: "tool.execute.after"` fields; the adapter envelope is the only
		// thing that carries the tool name and patchText. This forces the flush
		// path through `normalizeEventsForSessionContext` +
		// `projectAdapterToolEvent` and proves the adapter projection populates
		// `filesModified` via the `apply_patch` branch of `buildSessionContext`.
		const adapterEvent = {
			schema_version: "1.0",
			source: "opencode",
			session_id: sessionId,
			event_id: "oc:apply_patch:1",
			event_type: "tool_result",
			ts: "2026-04-11T12:00:05Z",
			ordering_confidence: "low",
			payload: {
				tool_name: "apply_patch",
				status: "ok",
				tool_input: { patchText },
				tool_output:
					"Success. Updated the following files:\nA /repo/src/new.ts\nU /repo/src/existing.ts",
				error: null,
			},
			meta: { original_event_type: "tool.execute.after" },
		};

		store.recordRawEvent({
			opencodeSessionId: sessionId,
			source: "opencode",
			eventId: "evt-prompt",
			eventType: "user_prompt",
			payload: {
				type: "user_prompt",
				prompt_text: "Ship the apply_patch fix",
				timestamp: "2026-04-11T12:00:00Z",
			},
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: Date.parse("2026-04-11T12:00:00Z"),
		});
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			source: "opencode",
			eventId: adapterEvent.event_id,
			eventType: "tool.execute.after",
			payload: { _adapter: adapterEvent },
			repositoryIdentity,
			sensitivity: "eligible",
			tsWallMs: Date.parse(adapterEvent.ts),
		});

		const summaryResponder = {
			observe: async () => ({
				raw: `<summary>
					<request>Ship the apply_patch fix</request>
					<investigated>raw-event-flush session context</investigated>
					<learned>apply_patch needs explicit handling</learned>
					<completed>Added handling</completed>
					<next_steps></next_steps>
					<notes></notes>
					<citations><cite source="0"/></citations>
				</summary>`,
				parsed: null,
				provider: "test",
				model: "test-model",
			}),
			getStatus: () => ({
				provider: "test",
				model: "test-model",
				runtime: "test",
				auth: { source: "none", type: "none", hasToken: false },
			}),
		};

		const ingestOpts = ingestOptions(summaryResponder);
		const flushOpts = {
			opencodeSessionId: sessionId,
			source: "opencode",
			cwd: "/repo",
			project: "repo",
			startedAt: "2026-04-11T12:00:00Z",
		};

		const result = await flushRawEvents(store, ingestOpts, flushOpts);
		expect(result.flushed).toBeGreaterThan(0);

		const row = store.db
			.prepare(
				"SELECT metadata_json FROM sessions WHERE id = (SELECT session_id FROM opencode_sessions WHERE stream_id = ?)",
			)
			.get(sessionId) as { metadata_json: string } | undefined;
		expect(row).toBeDefined();
		const meta = JSON.parse(row?.metadata_json ?? "{}") as {
			session_context?: {
				filesModified?: string[];
				toolCount?: number;
			};
		};
		expect(meta.session_context?.filesModified).toEqual([
			"/repo/src/existing.ts",
			"/repo/src/new.ts",
		]);
		expect(meta.session_context?.toolCount).toBe(1);
	});
});
