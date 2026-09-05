import type { MessagePort } from "node:worker_threads";
import {
	isMainThread,
	MessageChannel,
	parentPort,
	receiveMessageOnPort,
	Worker,
	workerData,
} from "node:worker_threads";
import {
	type ScanDetection,
	type ScannerOptions,
	type ScanResult,
	type SecretRule,
	SecretScanner,
} from "./secret-scanner.js";

export const REDACTION_WORKER_DEADLINE_MS = 100;
const REDACTION_WORKER_STARTUP_DEADLINE_MS = 1_000;
/**
 * How long a scan that fails closed will wait for the worker to come up, before its own
 * REDACTION_WORKER_DEADLINE_MS budget starts. Source and daemon callers use this allowance;
 * hook callers pass an earlier deadline that preserves their spool window. The wait is a
 * synchronous `Atomics.wait`, so it blocks the whole thread.
 */
const REDACTION_SCAN_STARTUP_BUDGET_MS = 500;

type WorkerRequest =
	| {
			type: "secret";
			value: unknown;
			rules: SecretRule[];
			allowlist: Array<string | RegExp>;
			parentKey?: string;
	  }
	| { type: "private"; value: unknown; patterns: string[] };

type WorkerResponse =
	| {
			ok: true;
			value: unknown;
			detections: ScanDetection[];
			privateHit: boolean;
	  }
	| { ok: false };

type RedactionWorkerData = { role: "redaction-worker"; ready: SharedArrayBuffer };
type WorkerMessage = {
	request: WorkerRequest;
	port: MessagePort;
	signal: SharedArrayBuffer;
};

let activeWorker: Worker | undefined;
let activeWorkerReady: Int32Array | undefined;
let activeWorkerStartedAt = 0;
let recentWorkerStartupFailureAt = Number.NEGATIVE_INFINITY;
let redactionWorkerRetrySuppressedUntilAtMs = Number.NEGATIVE_INFINITY;
const isHookRuntimeWorker =
	!isMainThread &&
	Boolean(workerData) &&
	typeof workerData === "object" &&
	(workerData as { role?: unknown }).role === "hook-runtime";

function isRedactionWorkerData(value: unknown): value is RedactionWorkerData {
	return (
		Boolean(value) &&
		typeof value === "object" &&
		(value as RedactionWorkerData).role === "redaction-worker" &&
		(value as RedactionWorkerData).ready instanceof SharedArrayBuffer
	);
}

function applyPrivateRegex(
	value: unknown,
	patterns: string[],
): { value: unknown; privateHit: boolean } {
	if (typeof value === "string") {
		let output = value;
		let privateHit = false;
		for (const source of patterns) {
			// This worker is terminated when the shared 100 ms redaction deadline expires.
			const pattern = new RegExp(source, "g"); // nosemgrep
			if (!pattern.test(output)) continue;
			privateHit = true;
			pattern.lastIndex = 0;
			output = output.replace(pattern, "");
		}
		return { value: output, privateHit };
	}
	if (Array.isArray(value)) {
		const output: unknown[] = [];
		let privateHit = false;
		for (const item of value) {
			const result = applyPrivateRegex(item, patterns);
			output.push(result.value);
			privateHit ||= result.privateHit;
		}
		return { value: output, privateHit };
	}
	if (value && typeof value === "object") {
		const output: Record<string, unknown> = {};
		let privateHit = false;
		for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
			const keyResult = applyPrivateRegex(key, patterns);
			if (keyResult.privateHit) {
				privateHit = true;
				continue;
			}
			const result = applyPrivateRegex(item, patterns);
			output[key] = result.value;
			privateHit ||= result.privateHit;
		}
		return { value: output, privateHit };
	}
	return { value, privateHit: false };
}

function handleWorkerMessage(message: WorkerMessage): void {
	const signal = new Int32Array(message.signal);
	let response: WorkerResponse;
	try {
		if (message.request.type === "private") {
			const result = applyPrivateRegex(message.request.value, message.request.patterns);
			response = { ok: true, detections: [], ...result };
		} else {
			const result = new SecretScanner({
				rules: message.request.rules,
				allowlist: message.request.allowlist,
			}).redactValue(message.request.value, message.request.parentKey);
			response = { ok: true, privateHit: false, ...result };
		}
	} catch {
		response = { ok: false };
	}
	message.port.postMessage(response);
	Atomics.store(signal, 0, 1);
	Atomics.notify(signal, 0);
	message.port.close();
}

if (!isMainThread && isRedactionWorkerData(workerData)) {
	parentPort?.on("message", handleWorkerMessage);
	const ready = new Int32Array(workerData.ready);
	Atomics.store(ready, 0, 1);
	Atomics.notify(ready, 0);
}

function getWorker(): Worker {
	if (!activeWorker) {
		const moduleUrl = new URL(import.meta.url);
		const ready = new Int32Array(new SharedArrayBuffer(4));
		const worker = new Worker(moduleUrl, {
			workerData: {
				role: "redaction-worker",
				ready: ready.buffer,
			} satisfies RedactionWorkerData,
			...(moduleUrl.pathname.endsWith(".ts") ? { execArgv: ["--import", "tsx"] } : {}),
		});
		activeWorker = worker;
		activeWorkerReady = ready;
		activeWorkerStartedAt = performance.now();
		worker.unref();
		worker.once("error", () => {
			if (activeWorker !== worker) return;
			activeWorker = undefined;
			activeWorkerReady = undefined;
			activeWorkerStartedAt = 0;
			Atomics.store(ready, 0, -1);
			Atomics.notify(ready, 0);
		});
		worker.once("exit", () => {
			if (activeWorker !== worker) return;
			activeWorker = undefined;
			activeWorkerReady = undefined;
			activeWorkerStartedAt = 0;
			Atomics.store(ready, 0, -1);
			Atomics.notify(ready, 0);
		});
	}
	return activeWorker;
}

/**
 * Starts the redaction worker if needed and waits for it to report ready.
 *
 * `deadlineAtMs` switches two things at once, so pick it deliberately:
 *
 * - the wait bound: `min(what is left of the worker's own startup window, your deadline)`,
 *   or the startup window alone when omitted;
 * - the discard policy: with a deadline, a worker that is merely slow is left booting, so
 *   the next call picks up where this one stopped. With no deadline, a worker that is not
 *   ready when the wait ends is terminated and the next call starts a fresh one.
 *
 * Pass a deadline whenever the caller has a budget to respect. Omit it only where blocking
 * for the full startup window is acceptable and a stuck worker is better replaced than kept
 * (process/daemon start-up).
 */
export function warmRedactionWorker(deadlineAtMs?: number): boolean {
	let worker: Worker;
	try {
		worker = getWorker();
	} catch {
		if (deadlineAtMs === undefined) recentWorkerStartupFailureAt = performance.now();
		return false;
	}
	// getWorker assigns the readiness view before it returns; worker error/exit callbacks
	// cannot run until this synchronous call stack yields.
	const ready = activeWorkerReady as Int32Array;
	if (Atomics.load(ready, 0) === 0) {
		const startupRemaining =
			REDACTION_WORKER_STARTUP_DEADLINE_MS - (performance.now() - activeWorkerStartedAt);
		const eventRemaining =
			deadlineAtMs === undefined ? startupRemaining : deadlineAtMs - performance.now();
		const waitMs = Math.floor(Math.min(startupRemaining, eventRemaining));
		if (waitMs > 0) Atomics.wait(ready, 0, 0, waitMs);
	}
	if (Atomics.load(ready, 0) === 1) return true;
	if (
		deadlineAtMs === undefined ||
		performance.now() - activeWorkerStartedAt >= REDACTION_WORKER_STARTUP_DEADLINE_MS
	) {
		discardWorker(worker);
	}
	if (deadlineAtMs === undefined) recentWorkerStartupFailureAt = performance.now();
	return false;
}

function redactionWorkerPreparationSuppressed(
	deadlineAtMs: number | undefined,
	now: number,
): boolean {
	if (deadlineAtMs !== undefined && now >= deadlineAtMs) {
		if (isHookRuntimeWorker && activeWorker && now >= redactionWorkerRetrySuppressedUntilAtMs) {
			redactionWorkerRetrySuppressedUntilAtMs = now + REDACTION_SCAN_STARTUP_BUDGET_MS;
		}
		return true;
	}
	return isHookRuntimeWorker && now < redactionWorkerRetrySuppressedUntilAtMs;
}

export function prepareRedactionWorkerForScan(deadlineAtMs?: number): number | null {
	const now = performance.now();
	if (redactionWorkerPreparationSuppressed(deadlineAtMs, now)) return null;
	const inCooldown = now - recentWorkerStartupFailureAt < REDACTION_SCAN_STARTUP_BUDGET_MS;
	if (inCooldown && !activeWorker) return null;
	const startupDeadlineAtMs =
		activeWorkerStartedAt > 0
			? activeWorkerStartedAt + REDACTION_SCAN_STARTUP_BUDGET_MS
			: now + REDACTION_SCAN_STARTUP_BUDGET_MS;
	const readinessDeadlineAtMs = Math.min(
		startupDeadlineAtMs,
		deadlineAtMs ?? Number.POSITIVE_INFINITY,
	);
	const ready = warmRedactionWorker(inCooldown ? now : Math.max(now, readinessDeadlineAtMs));
	if (ready) {
		recentWorkerStartupFailureAt = Number.NEGATIVE_INFINITY;
		const scanStartedAtMs = performance.now();
		if (deadlineAtMs !== undefined && scanStartedAtMs >= deadlineAtMs) return null;
		return Math.min(
			scanStartedAtMs + REDACTION_WORKER_DEADLINE_MS,
			deadlineAtMs === undefined
				? Number.POSITIVE_INFINITY
				: deadlineAtMs + REDACTION_WORKER_DEADLINE_MS,
		);
	}
	if (inCooldown) return null;
	if (
		activeWorker &&
		readinessDeadlineAtMs === startupDeadlineAtMs &&
		performance.now() + 1 >= startupDeadlineAtMs
	) {
		discardWorker(activeWorker);
	}
	recentWorkerStartupFailureAt = performance.now();
	return null;
}

if (isHookRuntimeWorker) {
	getWorker();
}

function discardWorker(worker: Worker): void {
	if (activeWorker === worker) {
		activeWorker = undefined;
		activeWorkerReady = undefined;
		activeWorkerStartedAt = 0;
	}
	void worker.terminate();
}

function runWorker(request: WorkerRequest, deadlineAtMs: number): WorkerResponse {
	const remaining = Math.floor(deadlineAtMs - performance.now());
	if (remaining < 1) return { ok: false };
	let worker: Worker;
	try {
		worker = getWorker();
	} catch {
		return { ok: false };
	}
	if (!activeWorkerReady || Atomics.load(activeWorkerReady, 0) !== 1) return { ok: false };
	const { port1, port2 } = new MessageChannel();
	const signal = new Int32Array(new SharedArrayBuffer(4));
	try {
		worker.postMessage({ request, port: port2, signal: signal.buffer }, [port2]);
		if (Atomics.wait(signal, 0, 0, remaining) === "timed-out") {
			discardWorker(worker);
			return { ok: false };
		}
		return (receiveMessageOnPort(port1)?.message as WorkerResponse | undefined) ?? { ok: false };
	} catch {
		discardWorker(worker);
		return { ok: false };
	} finally {
		port1.close();
	}
}

export function redactValueInWorker(
	value: unknown,
	userRules: SecretRule[],
	deadlineAtMs: number,
	allowlist: Array<string | RegExp> = [],
	parentKey?: string,
): WorkerResponse {
	return runWorker({ type: "secret", value, rules: userRules, allowlist, parentKey }, deadlineAtMs);
}

export function applyPrivateRegexInWorker(
	value: unknown,
	patterns: string[],
	deadlineAtMs: number,
): WorkerResponse {
	return runWorker({ type: "private", value, patterns }, deadlineAtMs);
}

export class RedactionWorkerError extends Error {
	constructor() {
		super("redaction worker deadline exceeded");
		this.name = "RedactionWorkerError";
	}
}

export class WorkerSecretScanner extends SecretScanner {
	private readonly options: ScannerOptions;

	constructor(scanner: SecretScanner) {
		super();
		this.options = scanner.workerOptions();
	}

	override scan(text: string): ScanResult {
		const result = this.scanValue(text);
		if (typeof result.value !== "string") throw new RedactionWorkerError();
		return { redacted: result.value, detections: result.detections };
	}

	override redactValue(
		value: unknown,
		parentKey?: string,
	): { value: unknown; detections: ScanDetection[] } {
		return this.scanValue(value, parentKey);
	}

	private scanValue(
		value: unknown,
		parentKey?: string,
	): { value: unknown; detections: ScanDetection[] } {
		if (this.options.degraded) throw new RedactionWorkerError();
		const deadlineAtMs = prepareRedactionWorkerForScan();
		if (deadlineAtMs === null) throw new RedactionWorkerError();
		const result = redactValueInWorker(
			value,
			this.options.rules ?? [],
			deadlineAtMs,
			this.options.allowlist,
			parentKey,
		);
		if (!result.ok) throw new RedactionWorkerError();
		return { value: result.value, detections: result.detections };
	}
}
