import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { compileDefaultCapabilityManifest, defaultResourceProfile } from "./capability-manifest.js";
import { connect } from "./db.js";
import type { IngestOptions } from "./ingest-pipeline.js";
import { RawEventSweeper } from "./raw-event-sweeper.js";
import type { MemoryStore } from "./store.js";
import { initTestSchema, openTestMemoryStore } from "./test-utils.js";

function sleep(ms: number) {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

describe("RawEventSweeper auto flush", () => {
	const manifest = compileDefaultCapabilityManifest({
		version: 1,
		role: "summary",
		state: "enabled",
		wireProtocol: "openai_chat_completions_v1",
		modelId: "raw-event-sweeper-test",
		modelRevision: "1",
		endpointUrl: "http://127.0.0.1:1234/v1/chat/completions",
		credentialRef: { kind: "none" },
	});
	const configurationFingerprint = manifest.configurationFingerprint;
	const providerFingerprint = manifest.summaryProvider.providerFingerprint;
	const repositoryIdentity = `repo-v1:sha256:${"c".repeat(64)}`;
	const frozenProviderOpts = {
		configurationFingerprint,
		providerFingerprint,
		capabilityManifest: manifest,
		providerTlsPeerVerified: false,
		resourceProfile: manifest.resourceProfile,
	};
	let tmpDir: string;
	let dbPath: string;
	let store: MemoryStore;
	let prevAutoFlush: string | undefined;
	let prevDebounce: string | undefined;
	let prevWorkerMaxEvents: string | undefined;

	beforeEach(() => {
		prevAutoFlush = process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH;
		prevDebounce = process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS;
		prevWorkerMaxEvents = process.env.CODEMEM_RAW_EVENTS_WORKER_MAX_EVENTS;
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-raw-event-sweeper-test-"));
		dbPath = join(tmpDir, "test.sqlite");
		const db = connect(dbPath);
		initTestSchema(db);
		db.close();
		store = openTestMemoryStore(dbPath);
	});

	afterEach(() => {
		store.close();
		if (prevAutoFlush == null) delete process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH;
		else process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = prevAutoFlush;
		if (prevDebounce == null) delete process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS;
		else process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = prevDebounce;
		if (prevWorkerMaxEvents == null) delete process.env.CODEMEM_RAW_EVENTS_WORKER_MAX_EVENTS;
		else process.env.CODEMEM_RAW_EVENTS_WORKER_MAX_EVENTS = prevWorkerMaxEvents;
		rmSync(tmpDir, { recursive: true, force: true });
	});

	function recordRawEvent(input: Parameters<MemoryStore["recordRawEvent"]>[0]) {
		return store.recordRawEvent({ sensitivity: "eligible", ...input, repositoryIdentity });
	}

	function seedSession(sessionId: string) {
		recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-0",
			eventType: "user_prompt",
			payload: { type: "user_prompt", prompt_text: "Hello from auto flush" },
			tsWallMs: 100,
		});
		recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-1",
			eventType: "tool.execute.after",
			payload: {
				type: "tool.execute.after",
				tool: "read",
				args: { filePath: "x" },
			},
			tsWallMs: 200,
		});
		store.updateRawEventSessionMeta({
			opencodeSessionId: sessionId,
			cwd: tmpDir,
			project: "codemem",
			startedAt: "2026-01-01T00:00:00Z",
			lastSeenTsWallMs: 200,
		});
	}

	function seedLifecycleOnlySession(sessionId: string) {
		recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-0",
			eventType: "session.started",
			payload: { type: "session.started" },
			tsWallMs: 100,
		});
		recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-1",
			eventType: "session.idle",
			payload: { type: "session.idle" },
			tsWallMs: 150,
		});
		recordRawEvent({
			opencodeSessionId: sessionId,
			eventId: "evt-2",
			eventType: "session.ended",
			payload: { type: "session.ended" },
			tsWallMs: 200,
		});
		store.updateRawEventSessionMeta({
			opencodeSessionId: sessionId,
			cwd: tmpDir,
			project: "codemem",
			startedAt: "2026-01-01T00:00:00Z",
			lastSeenTsWallMs: 200,
		});
	}

	function seedAdapterPromptSession(sessionId: string) {
		recordRawEvent({
			opencodeSessionId: sessionId,
			source: "claude",
			eventId: "evt-0",
			eventType: "claude.hook",
			payload: {
				type: "claude.hook",
				_adapter: {
					schema_version: "1.0",
					source: "claude",
					session_id: sessionId,
					event_id: "evt-0",
					event_type: "prompt",
					payload: { text: "Investigate a real issue", prompt_number: 1 },
					ts: "2026-01-01T00:00:00Z",
				},
			},
			tsWallMs: 100,
		});
		recordRawEvent({
			opencodeSessionId: sessionId,
			source: "claude",
			eventId: "evt-1",
			eventType: "claude.hook",
			payload: {
				type: "claude.hook",
				_adapter: {
					schema_version: "1.0",
					source: "claude",
					session_id: sessionId,
					event_id: "evt-1",
					event_type: "assistant",
					payload: { text: "I found the likely root cause." },
					ts: "2026-01-01T00:00:01Z",
				},
			},
			tsWallMs: 150,
		});
		store.updateRawEventSessionMeta({
			opencodeSessionId: sessionId,
			source: "claude",
			cwd: tmpDir,
			project: "codemem",
			startedAt: "2026-01-01T00:00:00Z",
			lastSeenTsWallMs: 150,
		});
	}

	const ingestOpts: IngestOptions = {
		...frozenProviderOpts,
		observer: {
			observe: async () => ({
				raw: `<summary>
  <request>Auto flush request</request>
  <completed>Flushed debounced raw events</completed>
	<citations><cite source="0"/></citations>
</summary>`,
				parsed: null,
				provider: "test",
				model: "test",
			}),
			getStatus: () => ({
				provider: "test",
				model: "test",
				runtime: "api_http",
				auth: { source: "test", type: "api_direct", hasToken: true },
			}),
		} as never,
	};

	it("suppresses auto flush during auth backoff after an auth failure", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedSession("sess-auth");
		let calls = 0;
		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => {
					calls += 1;
					const { ObserverAuthError } = await import("./observer-client.js");
					throw new ObserverAuthError("auth failed");
				},
				getStatus: () => ({
					provider: "test",
					model: "test",
					runtime: "api_http",
					auth: { source: "test", type: "api_direct", hasToken: true },
				}),
			} as never,
		});

		sweeper.nudge("sess-auth");
		await sleep(1_100);
		sweeper.nudge("sess-auth");
		await sleep(1_100);

		expect(calls).toBe(1);
	});

	it("waits for active auto flush work during stop", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedSession("sess-stop");
		let resolved = false;
		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => {
					await sleep(80);
					resolved = true;
					return {
						raw: `<summary><request>stop</request><completed>done</completed><citations><cite source="0"/></citations></summary>`,
						parsed: null,
						provider: "test",
						model: "test",
					};
				},
				getStatus: () => ({
					provider: "test",
					model: "test",
					runtime: "api_http",
					auth: { source: "test", type: "api_direct", hasToken: true },
				}),
			} as never,
		});

		sweeper.nudge("sess-stop");
		await sleep(1_050);
		await sweeper.stop();

		expect(resolved).toBe(true);
		expect(store.rawEventFlushState("sess-stop")).toBe(1);
	});

	it("uses the frozen manifest warm, periodic, idle, and debounce timing", () => {
		const sweeper = new RawEventSweeper(store, ingestOpts);
		const timing = sweeper as unknown as {
			intervalMs(): number;
			idleMs(): number;
			debounceMs(): number;
		};

		// Pin the frozen manifest values, then assert the sweeper reads exactly
		// those fields (intervalMs is periodicSweepIntervalMs, not the warm
		// lifetime it happens to equal).
		expect(manifest.resourceProfile.workerWarmLifetimeMs).toBe(30_000);
		expect(manifest.resourceProfile.periodicSweepIntervalMs).toBe(30_000);
		expect(manifest.resourceProfile.idleFlushMs).toBe(120_000);
		expect(manifest.resourceProfile.eventDebounceMs).toBe(1_000);
		expect(timing.intervalMs()).toBe(manifest.resourceProfile.periodicSweepIntervalMs);
		expect(timing.idleMs()).toBe(manifest.resourceProfile.idleFlushMs);
		expect(timing.debounceMs()).toBe(manifest.resourceProfile.eventDebounceMs);
	});

	it("waits in-flight work, leaves no orphan timer, and explicitly restarts", async () => {
		seedSession("sess-stop-fence");
		let releaseObserver = () => {};
		const observerGate = new Promise<void>((resolve) => {
			releaseObserver = resolve;
		});
		let observerEntered = () => {};
		const entered = new Promise<void>((resolve) => {
			observerEntered = resolve;
		});
		let calls = 0;
		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => {
					calls += 1;
					observerEntered();
					await observerGate;
					return {
						raw: `<summary><request>stop fence</request><completed>done</completed><citations><cite source="0"/></citations></summary>`,
						parsed: null,
						provider: "test",
						model: "test",
					};
				},
				getStatus: () => ({
					provider: "test",
					model: "test",
					runtime: "api_http",
					auth: { source: "none", type: "none", hasToken: false },
				}),
			} as never,
		});
		vi.useFakeTimers();
		let stopping: Promise<void> | undefined;
		try {
			sweeper.start();
			sweeper.nudge("sess-stop-fence");
			await vi.advanceTimersByTimeAsync(manifest.resourceProfile.eventDebounceMs);
			await entered;
			sweeper.nudge("sess-stop-fence");
			let stopped = false;
			stopping = sweeper.stop().then(() => {
				stopped = true;
			});
			await Promise.resolve();
			expect(stopped).toBe(false);

			releaseObserver();
			await stopping;
			expect(calls).toBe(1);
			expect(vi.getTimerCount()).toBe(0);
			await vi.advanceTimersByTimeAsync(manifest.resourceProfile.eventDebounceMs);
			expect(calls).toBe(1);

			seedSession("sess-restarted");
			sweeper.start();
			sweeper.nudge("sess-restarted");
			await vi.advanceTimersByTimeAsync(manifest.resourceProfile.eventDebounceMs);
			await sweeper.stop();
			expect(calls).toBe(2);
			expect(vi.getTimerCount()).toBe(0);
		} finally {
			releaseObserver();
			await stopping;
			await sweeper.stop();
			vi.useRealTimers();
		}
	});

	it("requeues activity that arrives during an active auto flush", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedSession("sess-rerun");
		let firstCall = true;
		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => {
					if (firstCall) {
						firstCall = false;
						recordRawEvent({
							opencodeSessionId: "sess-rerun",
							eventId: "evt-2",
							eventType: "tool.execute.after",
							payload: {
								type: "tool.execute.after",
								tool: "read",
								args: { filePath: "y" },
							},
							tsWallMs: 300,
						});
						store.updateRawEventSessionMeta({
							opencodeSessionId: "sess-rerun",
							cwd: tmpDir,
							project: "codemem",
							startedAt: "2026-01-01T00:00:00Z",
							lastSeenTsWallMs: 300,
						});
						sweeper.nudge("sess-rerun");
						await sleep(60);
					}
					return {
						raw: `<summary><request>rerun</request><completed>done</completed><citations><cite source="0"/></citations></summary>`,
						parsed: null,
						provider: "test",
						model: "test",
					};
				},
				getStatus: () => ({
					provider: "test",
					model: "test",
					runtime: "api_http",
					auth: { source: "test", type: "api_direct", hasToken: true },
				}),
			} as never,
		});

		sweeper.nudge("sess-rerun");
		await sleep(2_300);

		expect(store.rawEventFlushState("sess-rerun")).toBe(2);
	});

	it("ignores the retired auto-flush disable environment", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "0";
		seedSession("sess-disabled");
		const sweeper = new RawEventSweeper(store, ingestOpts);

		sweeper.nudge("sess-disabled");
		await sleep(1_100);

		expect(store.rawEventFlushState("sess-disabled")).toBe(1);
	});

	it("debounced auto flush advances flush state when enabled", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedSession("sess-auto");
		const sweeper = new RawEventSweeper(store, ingestOpts);

		sweeper.nudge("sess-auto");
		await sleep(1_100);

		expect(store.rawEventFlushState("sess-auto")).toBe(1);
	});

	it("does not postpone debounced auto flush forever during continued activity", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "40";
		seedSession("sess-bounded-debounce");
		const sweeper = new RawEventSweeper(store, ingestOpts);

		sweeper.nudge("sess-bounded-debounce");
		await sleep(20);
		recordRawEvent({
			opencodeSessionId: "sess-bounded-debounce",
			eventId: "evt-2",
			eventType: "assistant_message",
			payload: { type: "assistant_message", assistant_text: "still active" },
			tsWallMs: 300,
		});
		store.updateRawEventSessionMeta({
			opencodeSessionId: "sess-bounded-debounce",
			cwd: tmpDir,
			project: "codemem",
			startedAt: "2026-01-01T00:00:00Z",
			lastSeenTsWallMs: 300,
		});
		sweeper.nudge("sess-bounded-debounce");
		await sleep(1_100);

		expect(store.rawEventFlushState("sess-bounded-debounce")).toBeGreaterThanOrEqual(1);
	});

	it("uses the frozen 100-event source limit instead of legacy env", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		process.env.CODEMEM_RAW_EVENTS_WORKER_MAX_EVENTS = "2";
		seedSession("sess-small-batches");
		recordRawEvent({
			opencodeSessionId: "sess-small-batches",
			eventId: "evt-2",
			eventType: "assistant_message",
			payload: { type: "assistant_message", assistant_text: "a" },
			tsWallMs: 300,
		});
		recordRawEvent({
			opencodeSessionId: "sess-small-batches",
			eventId: "evt-3",
			eventType: "assistant_message",
			payload: { type: "assistant_message", assistant_text: "b" },
			tsWallMs: 400,
		});
		recordRawEvent({
			opencodeSessionId: "sess-small-batches",
			eventId: "evt-4",
			eventType: "assistant_message",
			payload: { type: "assistant_message", assistant_text: "c" },
			tsWallMs: 500,
		});
		store.updateRawEventSessionMeta({
			opencodeSessionId: "sess-small-batches",
			cwd: tmpDir,
			project: "codemem",
			startedAt: "2026-01-01T00:00:00Z",
			lastSeenTsWallMs: 500,
		});

		const sweeper = new RawEventSweeper(store, ingestOpts);
		sweeper.nudge("sess-small-batches");
		await sleep(1_100);

		expect(store.rawEventFlushState("sess-small-batches")).toBe(4);
	});

	it("terminally completes low-signal skip_summary batches and advances the flush cursor", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedSession("sess-low-signal");

		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => ({
					raw: '<skip_summary reason="low-signal"/>',
					parsed: null,
					provider: "test",
					model: "test",
				}),
				getStatus: () => ({
					provider: "test",
					model: "test",
					runtime: "api_http",
					auth: { source: "test", type: "api_direct", hasToken: true },
				}),
			} as never,
		});

		sweeper.nudge("sess-low-signal");
		await sleep(1_100);

		expect(store.rawEventFlushState("sess-low-signal")).toBe(1);
		expect(store.latestRawEventFlushFailure("opencode")?.stream_id).not.toBe("sess-low-signal");
	});

	it("records observer diagnostics for failed raw-event flushes", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedSession("sess-failed-diagnostics");

		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => ({
					raw: null,
					parsed: null,
					provider: "openai",
					model: "gpt-5.4-mini",
				}),
				getStatus: () => ({
					provider: "openai",
					model: "gpt-5.4-mini",
					runtime: "api_http",
					auth: { source: "env", type: "api_direct", hasToken: true },
					lastError: {
						code: "empty_response",
						message: "OpenAI returned 200 but response contained no extractable text.",
					},
				}),
			} as never,
		});

		sweeper.nudge("sess-failed-diagnostics");
		await sleep(1_100);

		const failure = store.latestRawEventFlushFailure("opencode");
		expect(failure?.stream_id).toBe("sess-failed-diagnostics");
		expect(failure).toMatchObject({
			observer_provider: "openai",
			observer_model: "gpt-5.4-mini",
			observer_runtime: "api_http",
			observer_auth_source: null,
			observer_auth_type: null,
			observer_error_code: "empty_response",
			observer_error_message: null,
			error_message: "OpenAI returned no usable output for raw-event processing.",
		});
	});

	it("terminally skips tiny lifecycle-only sessions without calling the observer", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedLifecycleOnlySession("sess-lifecycle-only");

		let observerCalls = 0;
		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => {
					observerCalls += 1;
					return {
						raw: '<skip_summary reason="low-signal"/>',
						parsed: null,
						provider: "test",
						model: "test",
					};
				},
				getStatus: () => ({
					provider: "test",
					model: "test",
					runtime: "api_http",
					auth: { source: "test", type: "api_direct", hasToken: true },
				}),
			} as never,
		});

		sweeper.nudge("sess-lifecycle-only");
		await sleep(1_100);

		expect(observerCalls).toBe(0);
		expect(store.rawEventFlushState("sess-lifecycle-only")).toBe(2);
		expect(store.latestRawEventFlushFailure("opencode")?.stream_id).not.toBe("sess-lifecycle-only");
	});

	it("keeps retention disabled despite retired config and env values", () => {
		const prevEnabled = process.env.CODEMEM_RAW_EVENTS_RETENTION_ENABLED;
		const prevMaxAge = process.env.CODEMEM_RAW_EVENTS_RETENTION_MAX_AGE_DAYS;
		const prevLegacy = process.env.CODEMEM_RAW_EVENTS_RETENTION_MS;
		// Isolate from any developer config file influence.
		const prevConfig = process.env.CODEMEM_CONFIG;
		process.env.CODEMEM_CONFIG = join(tmpDir, "no-such-config.json");
		// Access the private retentionMs() for direct assertion.
		const retentionMs = (s: RawEventSweeper) =>
			(s as unknown as { retentionMs(): number }).retentionMs();
		const sweeper = new RawEventSweeper(store, ingestOpts);
		try {
			// Retired config/env values cannot mutate the frozen profile.
			delete process.env.CODEMEM_RAW_EVENTS_RETENTION_MS;
			process.env.CODEMEM_RAW_EVENTS_RETENTION_ENABLED = "1";
			process.env.CODEMEM_RAW_EVENTS_RETENTION_MAX_AGE_DAYS = "30";
			expect(retentionMs(sweeper)).toBe(0);

			// 2. New key ABSENT => fall back to the legacy CODEMEM_RAW_EVENTS_RETENTION_MS env var.
			delete process.env.CODEMEM_RAW_EVENTS_RETENTION_ENABLED;
			delete process.env.CODEMEM_RAW_EVENTS_RETENTION_MAX_AGE_DAYS;
			process.env.CODEMEM_RAW_EVENTS_RETENTION_MS = "123456";
			expect(retentionMs(sweeper)).toBe(0);

			// 2b. EXPLICIT disable (enabled=0) is authoritative over a stale legacy
			// env var: retention stays off rather than silently honoring the legacy value.
			process.env.CODEMEM_RAW_EVENTS_RETENTION_ENABLED = "0";
			expect(retentionMs(sweeper)).toBe(0);
			delete process.env.CODEMEM_RAW_EVENTS_RETENTION_ENABLED;

			// 3. Neither set => 0 (no retention).
			delete process.env.CODEMEM_RAW_EVENTS_RETENTION_MS;
			expect(retentionMs(sweeper)).toBe(0);
		} finally {
			if (prevEnabled == null) delete process.env.CODEMEM_RAW_EVENTS_RETENTION_ENABLED;
			else process.env.CODEMEM_RAW_EVENTS_RETENTION_ENABLED = prevEnabled;
			if (prevMaxAge == null) delete process.env.CODEMEM_RAW_EVENTS_RETENTION_MAX_AGE_DAYS;
			else process.env.CODEMEM_RAW_EVENTS_RETENTION_MAX_AGE_DAYS = prevMaxAge;
			if (prevLegacy == null) delete process.env.CODEMEM_RAW_EVENTS_RETENTION_MS;
			else process.env.CODEMEM_RAW_EVENTS_RETENTION_MS = prevLegacy;
			if (prevConfig == null) delete process.env.CODEMEM_CONFIG;
			else process.env.CODEMEM_CONFIG = prevConfig;
		}
	});

	it("does not create a job for an idle stream with a retained source gap", async () => {
		const streamId = "sess-source-gap-private-path";
		for (let eventSeq = 0; eventSeq < 3; eventSeq++) {
			recordRawEvent({
				opencodeSessionId: streamId,
				eventId: `source-gap-event-${eventSeq}`,
				eventType: "message",
				payload: { type: "message", path: streamId },
				tsWallMs: eventSeq,
			});
		}
		store.db.prepare("DELETE FROM raw_events WHERE stream_id = ? AND event_seq = 1").run(streamId);
		store.updateRawEventSessionMeta({ opencodeSessionId: streamId, lastSeenTsWallMs: 0 });
		const sweeper = new RawEventSweeper(store, ingestOpts);

		await sweeper.tick();
		await sweeper.tick();

		expect(store.rawEventFlushState(streamId)).toBe(-1);
		expect(
			store.db
				.prepare("SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE stream_id = ?")
				.get(streamId),
		).toEqual({ count: 0 });
	});

	it("does not admit a fully pruned source gap from the event-driven nudge path", async () => {
		const streamId = "sess-nudge-source-gap-private-path";
		recordRawEvent({
			opencodeSessionId: streamId,
			eventId: "source-gap-event-0",
			eventType: "message",
			payload: { type: "message", path: streamId },
			tsWallMs: 0,
		});
		store.db.prepare("DELETE FROM raw_events WHERE stream_id = ?").run(streamId);
		vi.useFakeTimers();
		try {
			const admit = vi.spyOn(store, "admitRawEventFlushJob");
			const sweeper = new RawEventSweeper(store, ingestOpts);
			sweeper.nudge(streamId);
			await vi.advanceTimersByTimeAsync(defaultResourceProfile().eventDebounceMs);
			await sweeper.stop();
			expect(admit).toHaveBeenCalledWith(expect.objectContaining({ streamId }));
		} finally {
			vi.useRealTimers();
		}
		expect(store.rawEventFlushState(streamId)).toBe(-1);
		expect(
			store.db
				.prepare("SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE stream_id = ?")
				.get(streamId),
		).toEqual({ count: 0 });
	});

	it("does not retry a queued job whose retained source range has a gap", async () => {
		const streamId = "sess-queue-source-gap-private-path";
		for (let eventSeq = 0; eventSeq < 3; eventSeq++) {
			recordRawEvent({
				opencodeSessionId: streamId,
				eventId: `source-gap-event-${eventSeq}`,
				eventType: "message",
				payload: { type: "message", path: streamId },
				tsWallMs: eventSeq,
			});
		}
		const job = store.admitRawEventFlushJob({
			source: "opencode",
			streamId,
			manifestFingerprint: configurationFingerprint,
			providerFingerprint,
		});
		store.db.prepare("DELETE FROM raw_events WHERE stream_id = ? AND event_seq = 1").run(streamId);
		const sweeper = new RawEventSweeper(store, ingestOpts);

		await sweeper.tick();

		expect(store.rawEventFlushState(streamId)).toBe(-1);
		expect(
			store.db
				.prepare("SELECT status, attempt_count FROM raw_event_flush_batches WHERE id = ?")
				.get(job.jobId),
		).toEqual({ status: "queued", attempt_count: 0 });
	});

	it("flushes a valid queued range when only the later retained tail has a gap", async () => {
		const streamId = "sess-queue-tail-source-gap";
		seedSession(streamId);
		const job = store.admitRawEventFlushJob({
			source: "opencode",
			streamId,
			manifestFingerprint: configurationFingerprint,
			providerFingerprint,
		});
		for (let eventSeq = 2; eventSeq < 5; eventSeq++) {
			recordRawEvent({
				opencodeSessionId: streamId,
				eventId: `tail-event-${eventSeq}`,
				eventType: "message",
				payload: { type: "message" },
				tsWallMs: eventSeq * 100,
			});
		}
		store.db.prepare("DELETE FROM raw_events WHERE stream_id = ? AND event_seq = 3").run(streamId);
		const sweeper = new RawEventSweeper(store, ingestOpts);

		await sweeper.tick();

		expect(store.rawEventFlushState(streamId)).toBe(1);
		expect(
			store.db.prepare("SELECT status FROM raw_event_flush_batches WHERE id = ?").get(job.jobId),
		).toEqual({ status: "completed" });
	});

	it("does not terminally skip adapter-wrapped prompt sessions", async () => {
		process.env.CODEMEM_RAW_EVENTS_AUTO_FLUSH = "1";
		process.env.CODEMEM_RAW_EVENTS_DEBOUNCE_MS = "0";
		seedAdapterPromptSession("sess-adapter-prompt");

		let observerCalls = 0;
		const sweeper = new RawEventSweeper(store, {
			...frozenProviderOpts,
			observer: {
				observe: async () => {
					observerCalls += 1;
					return {
						raw: `<summary><request>Investigate a real issue</request><completed>Captured adapter wrapped session.</completed><citations><cite source="0"/></citations></summary>`,
						parsed: null,
						provider: "test",
						model: "test",
					};
				},
				getStatus: () => ({
					provider: "test",
					model: "test",
					runtime: "api_http",
					auth: { source: "test", type: "api_direct", hasToken: true },
				}),
			} as never,
		});

		sweeper.nudge("sess-adapter-prompt", "claude");
		await sleep(1_100);

		expect(observerCalls).toBe(1);
		expect(store.rawEventFlushState("sess-adapter-prompt", "claude")).toBe(1);
	});
});
