import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");

function runReadinessScenario(
	scenario:
		| "caller-deadline-local"
		| "cooldown"
		| "cooldown-expired"
		| "expired-ready"
		| "pipeline"
		| "prewarm-cooldown"
		| "store",
	readyDelayMs: number,
): unknown {
	const fixtureDir = mkdtempSync(join(tmpdir(), "codemem-redaction-readiness-"));
	const preloadPath = join(fixtureDir, "delay-redaction-worker.mjs");
	writeFileSync(
		preloadPath,
		`import { appendFileSync } from "node:fs";
import { isMainThread, workerData } from "node:worker_threads";
if (!isMainThread && workerData?.role === "redaction-worker") {
	if (
		process.env.CODEMEM_TEST_WORKER_FAIL_START === "1" ||
		process.env.CODEMEM_TEST_WORKER_STALL_START === "1"
	) {
		appendFileSync(process.env.CODEMEM_TEST_WORKER_SPAWN_LOG, "spawn\\n", { mode: 0o600 });
	}
	if (process.env.CODEMEM_TEST_WORKER_FAIL_START === "1") {
		throw new Error("injected redaction worker startup failure");
	}
	if (process.env.CODEMEM_TEST_WORKER_STALL_START === "1") {
		Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 60_000);
	}
	const delayMs = Number(process.env.CODEMEM_TEST_WORKER_READY_DELAY_MS ?? "0");
	if (delayMs > 0) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, delayMs);
}
`,
		{ mode: 0o600 },
	);
	const fixture = `
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as core from "./packages/core/src/index.ts";
import { WorkerSecretScanner } from "./packages/core/src/redaction-worker.ts";
import { SecretScanner } from "./packages/core/src/secret-scanner.ts";
import { insertTestSession, openTestMemoryStore } from "./packages/core/src/test-utils.ts";
const scenario = process.env.CODEMEM_TEST_SCENARIO;
if (scenario === "expired-ready") {
	if (!core.warmRedactionWorker()) throw new Error("fixture failed to warm the worker");
	const result = core.preprocessAdapterEvent(
		{ id: "expired", body: "must not scan" },
		{
			allowlist: ["id", "body"],
			metadataKeys: ["id"],
			workerStartupDeadlineAtMs: performance.now() - 1,
		},
	);
	console.log(JSON.stringify(result));
} else if (scenario === "caller-deadline-local") {
	const first = core.preprocessAdapterEvent(
		{ id: "bounded", body: "safe" },
		{
			allowlist: ["id", "body"],
			metadataKeys: ["id"],
			workerStartupDeadlineAtMs: performance.now() + 25,
		},
	);
	await new Promise((resolve) => setTimeout(resolve, 300));
	const second = core.preprocessAdapterEvent(
		{ id: "independent", body: "safe" },
		{ allowlist: ["id", "body"], metadataKeys: ["id"] },
	);
	console.log(JSON.stringify({ first, second }));
} else if (scenario === "pipeline") {
	const result = core.preprocessAdapterEvent(
		{ id: "cold", body: "safe" },
		{ allowlist: ["id", "body"], metadataKeys: ["id"] },
	);
	console.log(JSON.stringify(result));
} else if (scenario === "store") {
	const dir = mkdtempSync(join(tmpdir(), "codemem-redaction-store-"));
	process.env.CODEMEM_CONFIG = join(dir, "config.json");
	const store = openTestMemoryStore(join(dir, "test.sqlite"));
	try {
		const timeoutConfig = core.parseAgentMemoryToml('private_regex = ["(a+)+$"]');
		const timeout = core.preprocessAdapterEvent(
			{ id: "timeout", body: "a".repeat(26) + "!" },
			{ config: timeoutConfig, allowlist: ["id", "body"], metadataKeys: ["id"] },
		);
		if (!timeout.degraded) throw new Error("fixture failed to discard the worker");
		const sessionId = insertTestSession(store.db);
		const memoryId = store.remember(
			sessionId,
			"discovery",
			"Remembered title",
			"Remembered body",
			0.5,
			["kept"],
			{ note: "kept" },
		);
		console.log(JSON.stringify(store.get(memoryId)));
	} finally {
		store.close();
		rmSync(dir, { recursive: true, force: true });
	}
} else if (
	scenario === "cooldown" ||
	scenario === "cooldown-expired" ||
	scenario === "prewarm-cooldown"
) {
	const scanner = new WorkerSecretScanner(new SecretScanner());
	if (scenario === "prewarm-cooldown") core.warmRedactionWorker();
	const attempts = scenario === "cooldown-expired" ? 3 : scenario === "prewarm-cooldown" ? 1 : 2;
	for (let index = 0; index < attempts; index += 1) {
		try {
			scanner.scan("safe");
		} catch {
			// Expected while the injected worker cannot become ready.
		}
		const pauseMs = scenario === "cooldown-expired" && index === 1 ? 550 : 50;
		await new Promise((resolve) => setTimeout(resolve, pauseMs));
	}
	const spawnLog = process.env.CODEMEM_TEST_WORKER_SPAWN_LOG;
	const spawnCount = spawnLog && existsSync(spawnLog)
		? readFileSync(spawnLog, "utf8").trim().split("\\n").filter(Boolean).length
		: 0;
	console.log(JSON.stringify({ spawnCount }));
} else {
	throw new Error("unknown scenario: " + scenario);
}
`;
	try {
		const preloadOption = `--import=${pathToFileURL(preloadPath).href}`;
		const nodeOptions = [process.env.NODE_OPTIONS, preloadOption].filter(Boolean).join(" ");
		const result = spawnSync(
			process.execPath,
			["--import", "tsx", "--input-type=module", "--eval", fixture],
			{
				cwd: workspaceRoot,
				encoding: "utf8",
				env: {
					...process.env,
					CODEMEM_TEST_SCENARIO: scenario,
					CODEMEM_TEST_WORKER_FAIL_START:
						scenario === "cooldown" || scenario === "prewarm-cooldown" ? "1" : "0",
					CODEMEM_TEST_WORKER_STALL_START: scenario === "cooldown-expired" ? "1" : "0",
					CODEMEM_TEST_WORKER_READY_DELAY_MS: String(readyDelayMs),
					CODEMEM_TEST_WORKER_SPAWN_LOG: join(fixtureDir, "worker-spawns.log"),
					NODE_OPTIONS: nodeOptions,
				},
				timeout: 10_000,
			},
		);
		expect(result.error).toBeUndefined();
		expect(result.status, result.stderr).toBe(0);
		return JSON.parse(result.stdout.trim()) as unknown;
	} finally {
		rmSync(fixtureDir, { recursive: true, force: true });
	}
}

describe("redaction worker readiness", () => {
	it("does not scan after the caller readiness deadline expires", () => {
		const result = runReadinessScenario("expired-ready", 0) as {
			degraded: boolean;
			payload: Record<string, unknown>;
		};
		expect(result.degraded).toBe(true);
		expect(result.payload).toEqual({ id: "expired" });
	});

	it("keeps a caller deadline local to that redaction attempt", () => {
		const result = runReadinessScenario("caller-deadline-local", 125) as {
			first: { degraded: boolean };
			second: { degraded: boolean; payload: Record<string, unknown> };
		};
		expect(result.first.degraded).toBe(true);
		expect(result.second.degraded).toBe(false);
		expect(result.second.payload).toMatchObject({ id: "independent", body: "safe" });
	});

	it("keeps adapter content when a cold worker becomes ready inside its readiness budget", () => {
		const result = runReadinessScenario("pipeline", 300) as {
			degraded: boolean;
			payload: Record<string, unknown>;
		};
		expect(result.degraded).toBe(false);
		expect(result.payload).toMatchObject({ id: "cold", body: "safe" });
	});

	it("keeps memory content when a scan timeout makes the next worker cold", () => {
		const row = runReadinessScenario("store", 300) as {
			title: string;
			body_text: string;
			tags_text: string;
			metadata_json: Record<string, unknown>;
		};
		expect(row).toMatchObject({
			title: "Remembered title",
			body_text: "Remembered body",
			tags_text: "kept",
			metadata_json: { note: "kept" },
		});
	}, 15_000);

	it("suppresses replacement worker starts during the readiness cooldown", () => {
		const result = runReadinessScenario("cooldown", 0) as { spawnCount: number };
		expect(result.spawnCount).toBe(1);
	});

	it("suppresses a replacement after a failed process prewarm", () => {
		const result = runReadinessScenario("prewarm-cooldown", 0) as { spawnCount: number };
		expect(result.spawnCount).toBe(1);
	});

	it("starts one replacement after an expired worker's cooldown", () => {
		const result = runReadinessScenario("cooldown-expired", 0) as { spawnCount: number };
		expect(result.spawnCount).toBe(2);
	});
});
