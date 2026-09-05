import { afterEach, describe, expect, it, vi } from "vitest";

const workerState = vi.hoisted(() => ({
	mode: "ready" as "ready" | "ready-after-deadline" | "stalled" | "throw",
	spawnCount: 0,
	terminateCount: 0,
	clockMs: 0,
}));

vi.mock("node:worker_threads", async (importOriginal) => {
	const actual = await importOriginal<typeof import("node:worker_threads")>();
	class FakeWorker {
		constructor(_filename: URL, options: { workerData?: { ready?: SharedArrayBuffer } } = {}) {
			workerState.spawnCount++;
			if (workerState.mode === "throw") throw new Error("injected worker constructor failure");
			const readyBuffer = options.workerData?.ready;
			if (
				(workerState.mode === "ready" || workerState.mode === "ready-after-deadline") &&
				readyBuffer
			) {
				const ready = new Int32Array(readyBuffer);
				Atomics.store(ready, 0, 1);
				Atomics.notify(ready, 0);
			}
		}

		unref(): this {
			if (workerState.mode === "ready-after-deadline") workerState.clockMs += 30;
			return this;
		}

		once(_event: string, _handler: (...args: unknown[]) => void): this {
			return this;
		}

		postMessage(): void {}

		terminate(): Promise<number> {
			workerState.terminateCount++;
			return Promise.resolve(0);
		}
	}
	return { ...actual, Worker: FakeWorker };
});

async function loadWorker(mode: typeof workerState.mode) {
	vi.resetModules();
	workerState.mode = mode;
	workerState.spawnCount = 0;
	workerState.terminateCount = 0;
	workerState.clockMs = 0;
	vi.spyOn(performance, "now").mockImplementation(() => workerState.clockMs);
	vi.spyOn(Atomics, "wait").mockImplementation((_view, _index, _value, timeout) => {
		workerState.clockMs += typeof timeout === "number" ? timeout : 0;
		return "timed-out";
	});
	return import("./redaction-worker.js");
}

afterEach(() => {
	vi.restoreAllMocks();
});

describe("redaction worker lifecycle failures", () => {
	it("records a constructor failure and suppresses an immediate replacement", async () => {
		const worker = await loadWorker("throw");
		expect(worker.warmRedactionWorker()).toBe(false);
		expect(worker.prepareRedactionWorkerForScan()).toBeNull();
		expect(workerState.spawnCount).toBe(1);
	});

	it("does not retain a caller-bounded constructor failure as a prewarm cooldown", async () => {
		const worker = await loadWorker("throw");
		const deadlineAtMs = performance.now() + 25;
		expect(worker.warmRedactionWorker(deadlineAtMs)).toBe(false);
		expect(worker.prepareRedactionWorkerForScan(deadlineAtMs)).toBeNull();
		expect(workerState.spawnCount).toBe(2);
	});

	it("discards an unbounded prewarm that never becomes ready", async () => {
		const worker = await loadWorker("stalled");
		expect(worker.warmRedactionWorker()).toBe(false);
		expect(workerState.terminateCount).toBe(1);
	});

	it("discards an expired readiness attempt and starts one worker after cooldown", async () => {
		const worker = await loadWorker("stalled");
		expect(worker.prepareRedactionWorkerForScan()).toBeNull();
		expect(workerState.terminateCount).toBe(1);
		expect(worker.prepareRedactionWorkerForScan()).toBeNull();
		expect(workerState.spawnCount).toBe(1);

		workerState.clockMs += 501;
		workerState.mode = "ready";
		const scanDeadlineAtMs = worker.prepareRedactionWorkerForScan();
		expect(scanDeadlineAtMs).not.toBeNull();
		expect(workerState.spawnCount).toBe(2);
	});

	it("does not start a worker after the caller readiness deadline", async () => {
		const worker = await loadWorker("ready");
		expect(worker.prepareRedactionWorkerForScan(performance.now() - 1)).toBeNull();
		expect(workerState.spawnCount).toBe(0);
		expect(worker.prepareRedactionWorkerForScan()).not.toBeNull();
		expect(workerState.spawnCount).toBe(1);
	});

	it("uses cooldown after a caller deadline prevents readiness", async () => {
		const worker = await loadWorker("stalled");
		expect(worker.prepareRedactionWorkerForScan(performance.now() + 25)).toBeNull();
		expect(workerState.terminateCount).toBe(0);
		expect(worker.prepareRedactionWorkerForScan(performance.now() + 25)).toBeNull();
		expect(workerState.spawnCount).toBe(1);
	});

	it("does not scan when readiness arrives after the caller deadline", async () => {
		const worker = await loadWorker("ready-after-deadline");
		expect(worker.prepareRedactionWorkerForScan(performance.now() + 25)).toBeNull();
		expect(workerState.spawnCount).toBe(1);
	});

	it("caps the scan end at the caller readiness deadline plus the scan budget", async () => {
		const worker = await loadWorker("ready");
		const readinessDeadlineAtMs = performance.now() + 25;
		const scanDeadlineAtMs = worker.prepareRedactionWorkerForScan(readinessDeadlineAtMs);
		expect(scanDeadlineAtMs).not.toBeNull();
		expect(scanDeadlineAtMs as number).toBeLessThanOrEqual(readinessDeadlineAtMs + 100);
	});
});
