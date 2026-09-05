import { join } from "node:path";
import { resolveRuntimeDataDir } from "@codemem/core";
import type { McpRpcOutcome } from "@codemem/mcp";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
	boundAttention,
	collectStatusReport,
	createStatusCommand,
	type OperationalStatusReport,
	renderStatusReport,
	type StatusDependencies,
} from "./status.js";

const daemonUnavailable: McpRpcOutcome = {
	ok: false,
	error: {
		code: "daemon_unavailable",
		message: "The local memory daemon is unavailable.",
		retryable: true,
	},
};

function dependencies(
	overrides: Partial<StatusDependencies> = {},
): StatusDependencies & { stdout: string[]; stderr: string[]; exitCodes: number[] } {
	const stdout: string[] = [];
	const stderr: string[] = [];
	const exitCodes: number[] = [];
	return {
		now: () => new Date("2026-08-14T00:00:00.000Z"),
		exists: () => false,
		readText: () => null,
		requestRpc: async () => daemonUnavailable,
		fetch: vi.fn(async () => {
			throw new Error("viewer unavailable");
		}) as typeof fetch,
		isProcessRunning: () => false,
		env: {},
		writeStdout: (text) => stdout.push(text),
		writeStderr: (text) => stderr.push(text),
		setExitCode: (code) => exitCodes.push(code),
		...overrides,
		stdout,
		stderr,
		exitCodes,
	};
}

afterEach(() => {
	process.exitCode = 0;
	vi.restoreAllMocks();
});

describe("status command", () => {
	it("emits one structured error and exits one on collection failure", async () => {
		const deps = dependencies({
			requestRpc: async () => {
				throw new Error("broken RPC");
			},
		});
		await createStatusCommand(deps).parseAsync(["--json"], { from: "user" });

		expect(deps.stdout).toEqual([
			JSON.stringify({ error: "status_failed", message: "Unable to collect operational status" }),
		]);
		expect(deps.exitCodes).toEqual([1]);
	});

	it("rejects positional arguments as usage errors", async () => {
		const deps = dependencies();
		const command = createStatusCommand(deps);
		await command.parseAsync(["unexpected", "--json"], { from: "user" });

		expect(deps.exitCodes).toEqual([2]);
		expect(command.options.map((option) => option.long)).toEqual(
			expect.arrayContaining(["--db-path", "--config", "--json"]),
		);
	});

	it("caps and bounds attention entries", async () => {
		const attention = boundAttention(
			Array.from({ length: 25 }, (_, index) => ({
				code: `unsafe-${index}`,
				severity: "warning" as const,
				message: "x".repeat(600),
			})),
		);
		expect(attention).toHaveLength(20);
		expect(attention[0]?.message).toHaveLength(500);
		const report: OperationalStatusReport = {
			checked_at: "2026-08-14T00:00:00.000Z",
			ok: false,
			version: "test",
			daemon: { state: "not_running" },
			database: { state: "missing" },
			runtime: { viewer: "unreachable" },
			maintenance: { state: "unknown" },
			semantic_index: { state: "unknown" },
			raw_events: { state: "unknown", pending: 0, source_gaps: 0 },
			processing_jobs: {
				capacity: 25,
				uncompleted: 2,
				processing: 0,
				failed: 0,
				exhausted: 2,
				pending_grants: 0,
				max_attempt: 0,
				legacy_unrecoverable: 0,
				retry_exhausted_job_ids: [3, 7],
				next_action: "confirm_retry",
			},
			observer: { state: "unconfigured" },
			capability: null,
			attention: [],
		};
		const rendered = renderStatusReport(report);
		expect(rendered).toContain("Database:       missing");
		expect(rendered).toContain("Viewer:         unreachable\nMaintenance:");
		expect(rendered).toContain("Retry job IDs:  3, 7");
		expect(renderStatusReport({ ...report, runtime: { viewer: "running", pid: 42 } })).toContain(
			"Viewer:         running (pid 42)",
		);

		const previousDataDir = process.env.CODEMEM_DATA_DIR;
		delete process.env.CODEMEM_DATA_DIR;
		try {
			const dbPaths = ["/tmp/codemem/a.sqlite", "/tmp/codemem/b.sqlite"];
			const pidPaths: string[] = [];
			const fetchMock = vi.fn<typeof fetch>();
			const deps = dependencies({
				exists: (path) => {
					pidPaths.push(path);
					return false;
				},
				fetch: fetchMock,
			});
			for (const dbPath of dbPaths) {
				await createStatusCommand(deps).parseAsync(["--db-path", dbPath, "--json"], {
					from: "user",
				});
			}
			expect(pidPaths).toEqual(
				dbPaths.map((dbPath) => join(resolveRuntimeDataDir({ dbPath }), "viewer.pid")),
			);
			expect(new Set(pidPaths).size).toBe(2);
			expect(deps.stdout.map((text) => JSON.parse(text).runtime.viewer)).toEqual([
				"stopped",
				"stopped",
			]);
			expect(fetchMock).not.toHaveBeenCalled();
		} finally {
			if (previousDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
			else process.env.CODEMEM_DATA_DIR = previousDataDir;
		}
	});

	it("reports the frozen capability identity and pending boundaries", async () => {
		const capability = {
			mode: "configured",
			configurationFingerprint: `sha256:${"a".repeat(64)}`,
			runtimeReason: "pending_privacy_boundary",
			providerEnabled: false,
			schemaReadiness: "pending_schema_v21",
			packReadiness: "pending_pack_boundary",
			summaryProvider: { providerFingerprint: `sha256:${"b".repeat(64)}` },
		};
		const operationalStatus = {
			maintenance: { state: "idle", running: 0, failed: 0 },
			semantic_index: { state: "degraded", vector_table_present: true },
			raw_events: { available: true, pending: 0, source_gaps: 0, failed_batches: 0 },
			observer: { available: true, failed_batches: 0, backoff_batches: 0 },
		};
		const deps = dependencies({
			requestRpc: async (_dataDir, method) =>
				method === "GET /v1/health"
					? { ok: true, result: { status: "ok", capability } }
					: {
							ok: true,
							result: { diagnostics: { operationalStatus, capability } },
						},
		});

		const report = await collectStatusReport({}, deps);

		expect(report.capability).toEqual(capability);
		expect(report.observer.state).toBe("pending");
		const rendered = renderStatusReport(report);
		expect(rendered).toContain(capability.configurationFingerprint);
		expect(rendered).toContain("pending_schema_v21");
		expect(rendered).toContain("pending_pack_boundary");
	});

	it("reports source gaps without projecting stream or payload details", async () => {
		const sensitive = "source-gap-private-path";
		const operationalStatus = {
			maintenance: { state: "idle", running: 0, failed: 0 },
			semantic_index: { state: "healthy", vector_table_present: true },
			raw_events: {
				available: true,
				pending: 3,
				source_gaps: 100,
				failed_batches: 0,
				source_gap_detail: sensitive,
			},
			processing_jobs: { next_action: "upgrade_runtime" },
			observer: { available: true, failed_batches: 0, backoff_batches: 0 },
		};
		const deps = dependencies({
			requestRpc: async (_dataDir, method) =>
				method === "GET /v1/health"
					? { ok: true, result: { status: "ok" } }
					: { ok: true, result: { diagnostics: { operationalStatus } } },
		});

		const report = await collectStatusReport({}, deps);

		expect(report.raw_events).toEqual({ state: "failing", pending: 3, source_gaps: 25 });
		expect(report.processing_jobs.next_action).toBe("upgrade_runtime");
		expect(report.attention).toContainEqual({
			code: "raw_events_source_gap",
			severity: "error",
			message: "25 raw-event source gaps detected in bounded scan; upgrade the runtime",
		});
		expect(renderStatusReport(report)).toContain(
			"Raw events:     failing (3 pending, 25 source gaps (bounded scan))",
		);
		expect(
			renderStatusReport({ ...report, raw_events: { ...report.raw_events, source_gaps: 1 } }),
		).toContain("Raw events:     failing (3 pending, 1 source gap (bounded scan))");
		expect(JSON.stringify(report)).not.toContain(sensitive);
	});

	it("directs exhausted jobs without a retry target to setup", async () => {
		const operationalStatus = {
			maintenance: { state: "idle", running: 0, failed: 0 },
			semantic_index: { state: "healthy", vector_table_present: true },
			raw_events: { available: true, pending: 0, source_gaps: 0, failed_batches: 0 },
			processing_jobs: {
				exhausted: 1,
				retry_exhausted_job_ids: [7],
				next_action: "activate_valid_manifest",
			},
			observer: { available: true, failed_batches: 0, backoff_batches: 0 },
		};
		const deps = dependencies({
			requestRpc: async (_dataDir, method) =>
				method === "GET /v1/health"
					? { ok: true, result: { status: "ok" } }
					: { ok: true, result: { diagnostics: { operationalStatus } } },
		});

		const report = await collectStatusReport({}, deps);

		expect(report.attention).toContainEqual({
			code: "processing_jobs_exhausted",
			severity: "error",
			message:
				"Raw-event processing has exhausted retries; run `codemem setup` to activate a valid manifest before retrying",
		});
		expect(JSON.stringify(report.attention)).not.toContain("confirm one exact retry");
	});

	it("keeps unavailable raw-event diagnostics unknown and fails closed", async () => {
		for (const [rawEvents, expectedSourceGaps] of [
			[{ available: true, pending: 3, failed_batches: 0 }, 0],
			[{ available: false, pending: 3, source_gaps: 4, failed_batches: 0 }, 4],
		] as const) {
			const operationalStatus = {
				maintenance: { state: "idle", running: 0, failed: 0 },
				semantic_index: { state: "healthy", vector_table_present: true },
				raw_events: rawEvents,
				processing_jobs: {
					exhausted: 1,
					retry_exhausted_job_ids: [7],
					next_action: "confirm_retry",
				},
				observer: { available: true, failed_batches: 0, backoff_batches: 0 },
			};
			const deps = dependencies({
				requestRpc: async (_dataDir, method) =>
					method === "GET /v1/health"
						? { ok: true, result: { status: "ok" } }
						: { ok: true, result: { diagnostics: { operationalStatus } } },
			});

			const report = await collectStatusReport({}, deps);

			expect(report.raw_events).toEqual({
				state: "unknown",
				pending: 3,
				source_gaps: expectedSourceGaps,
			});
			expect(report.processing_jobs.next_action).toBe("upgrade_runtime");
			expect(report.processing_jobs.retry_exhausted_job_ids).toEqual([]);
			expect(report.attention).toContainEqual({
				code: "raw_events_unavailable",
				severity: "error",
				message: "Raw-event diagnostics are unavailable; upgrade the runtime",
			});
			if (expectedSourceGaps > 0) {
				expect(report.attention).toContainEqual({
					code: "raw_events_source_gap",
					severity: "error",
					message: `${expectedSourceGaps} raw-event source gaps detected in bounded scan; upgrade the runtime`,
				});
			}
			expect(report.ok).toBe(false);
			expect(report.attention).toContainEqual({
				code: "processing_jobs_exhausted",
				severity: "error",
				message: "Raw-event processing has exhausted retries; upgrade the runtime before retrying",
			});
		}
	});

	it("keeps health capability when doctor is unavailable", async () => {
		const capability = {
			mode: "configured",
			configurationFingerprint: `sha256:${"a".repeat(64)}`,
			runtimeReason: "pending_privacy_boundary",
			providerEnabled: false,
			providerHealth: "provider_unavailable",
			schemaReadiness: "pending_schema_v21",
			packReadiness: "pending_pack_boundary",
			summaryProvider: { providerFingerprint: `sha256:${"b".repeat(64)}` },
		};
		const deps = dependencies({
			requestRpc: async (_dataDir, method) =>
				method === "GET /v1/health"
					? { ok: true, result: { status: "ok", capability } }
					: {
							ok: false,
							error: {
								code: "database_unavailable",
								message: "Database could not be read",
								retryable: true,
							},
						},
		});

		const report = await collectStatusReport({}, deps);

		expect(report.database.state).toBe("unavailable");
		expect(report.capability).toEqual(capability);
		const rendered = renderStatusReport(report);
		expect(rendered).toContain("Capability:     pending_privacy_boundary");
		expect(rendered).toContain(capability.configurationFingerprint);
		expect(rendered).toContain(capability.summaryProvider.providerFingerprint);
		expect(rendered).toContain("Provider health: provider_unavailable");
		expect(rendered).toContain("pending_schema_v21");
		expect(rendered).toContain("pending_pack_boundary");

		await createStatusCommand(deps).parseAsync(["--json"], { from: "user" });
		expect(JSON.parse(deps.stdout[0] ?? "")).toMatchObject({
			database: { state: "unavailable" },
			capability,
		});
	});

	it("only displays exact positive integer retry job IDs", async () => {
		const operationalStatus = {
			maintenance: { state: "idle", running: 0, failed: 0 },
			semantic_index: { state: "healthy", vector_table_present: true },
			raw_events: { available: true, pending: 0, source_gaps: 0, failed_batches: 0 },
			processing_jobs: {
				exhausted: 2,
				retry_exhausted_job_ids: [4, 1.9, "2", true, 3, 4],
				next_action: "confirm_retry",
			},
			observer: { available: true, failed_batches: 0, backoff_batches: 0 },
		};
		const deps = dependencies({
			requestRpc: async (_dataDir, method) =>
				method === "GET /v1/health"
					? { ok: true, result: { status: "ok" } }
					: { ok: true, result: { diagnostics: { operationalStatus } } },
		});

		const report = await collectStatusReport({}, deps);

		expect(report.processing_jobs.retry_exhausted_job_ids).toEqual([3, 4]);
		expect(renderStatusReport(report)).toContain("Retry job IDs:  3, 4");
	});

	it("uses safe fallbacks for malformed capability text from doctor RPC", async () => {
		const capability = {
			mode: {},
			configurationFingerprint: {},
			runtimeReason: {},
			providerFingerprint: {},
			schemaReadiness: {},
			packReadiness: {},
			summaryProvider: { providerFingerprint: {} },
		};
		const operationalStatus = {
			maintenance: { state: "idle", running: 0, failed: 0 },
			semantic_index: { state: "healthy", vector_table_present: true },
			raw_events: { available: true, pending: 0, source_gaps: 0, failed_batches: 0 },
			observer: { available: true, failed_batches: 0, backoff_batches: 0 },
		};
		const deps = dependencies({
			requestRpc: async (_dataDir, method) =>
				method === "GET /v1/health"
					? { ok: true, result: { status: "ok" } }
					: { ok: true, result: { diagnostics: { operationalStatus, capability } } },
		});

		const rendered = renderStatusReport(await collectStatusReport({}, deps));

		expect(rendered).toContain("Capability:     unknown");
		expect(rendered).toContain("Manifest:       none");
		expect(rendered).toContain("Provider:       none");
		expect(rendered).toContain("Schema:         unknown");
		expect(rendered).toContain("Pack:           unknown");
		expect(rendered).not.toContain("[object Object]");
	});
});
