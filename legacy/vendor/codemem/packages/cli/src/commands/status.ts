import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import {
	defaultResourceProfile,
	type OperationalNextAction,
	type OperationalProcessingJobs,
	type OperationalStatusSnapshot,
	VERSION,
} from "@codemem/core";
import { createMcpRpcClient, type McpRpcOutcome } from "@codemem/mcp";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import {
	addConfigOption,
	addDbOption,
	addJsonOption,
	type ConfigOpts,
	type DbOpts,
	type JsonOpts,
	resolveDataDirOpt,
} from "../shared-options.js";
import {
	observeViewerRuntime,
	parseViewerPidRecord,
	type ViewerRuntimeObservation,
} from "../viewer-runtime.js";

export type DatabaseState = "ready" | "missing" | "unavailable" | "unknown";
export type DaemonState = "running" | "not_running" | "unavailable";
export type MaintenanceState = "idle" | "running" | "failed" | "unknown";
export type SemanticIndexState = "healthy" | "pending" | "degraded" | "failed" | "unknown";
export type RawEventsState = "healthy" | "backlogged" | "failing" | "unknown";
export type ObserverState =
	| "healthy"
	| "idle"
	| "pending"
	| "backoff"
	| "failed"
	| "unconfigured"
	| "unknown";
export type AttentionSeverity = "warning" | "error";

export interface StatusAttention {
	code: string;
	severity: AttentionSeverity;
	message: string;
}

export interface OperationalStatusReport {
	checked_at: string;
	ok: boolean;
	version: string;
	daemon: { state: DaemonState };
	database: { state: DatabaseState };
	runtime: { viewer: ViewerRuntimeObservation["state"]; pid?: number };
	maintenance: { state: MaintenanceState };
	semantic_index: { state: SemanticIndexState };
	raw_events: { state: RawEventsState; pending: number; source_gaps: number };
	processing_jobs: OperationalProcessingJobs;
	observer: { state: ObserverState };
	capability: Record<string, unknown> | null;
	attention: StatusAttention[];
}

interface StatusOptions extends DbOpts, ConfigOpts, JsonOpts {}

const PROCESSING_JOB_CAPACITY = defaultResourceProfile().processingQueueCapacity;

export interface StatusDependencies {
	now: () => Date;
	exists: (path: string) => boolean;
	readText: (path: string) => string | null;
	requestRpc: (
		dataDir: string,
		method: "GET /v1/health" | "GET /v1/doctor",
	) => Promise<McpRpcOutcome>;
	fetch: typeof fetch;
	isProcessRunning: (pid: number) => boolean | null;
	env: NodeJS.ProcessEnv;
	writeStdout: (text: string) => void;
	writeStderr: (text: string) => void;
	setExitCode: (code: number) => void;
}

const MAX_ATTENTION = 20;
const DEFAULT_VIEWER_PORT = 38_888;
const defaultDependencies: StatusDependencies = {
	now: () => new Date(),
	exists: existsSync,
	readText: (path) => {
		try {
			return readFileSync(path, "utf8");
		} catch {
			return null;
		}
	},
	requestRpc: (dataDir, method) => createMcpRpcClient({ dataDir }).request(method, {}),
	fetch,
	isProcessRunning: (pid) => {
		try {
			process.kill(pid, 0);
			return true;
		} catch (error) {
			if (
				typeof error === "object" &&
				error !== null &&
				"code" in error &&
				error.code === "EPERM"
			) {
				return null;
			}
			return false;
		}
	},
	env: process.env,
	writeStdout: (text) => console.log(text),
	writeStderr: (text) => console.error(text),
	setExitCode: (code) => {
		process.exitCode = code;
	},
};

function viewerDefaultTarget(env: NodeJS.ProcessEnv): { host: string; port: number } {
	const host = env.CODEMEM_VIEWER_HOST?.trim() || "127.0.0.1";
	const parsedPort = Number.parseInt(env.CODEMEM_VIEWER_PORT ?? "", 10);
	return {
		host,
		port:
			Number.isFinite(parsedPort) && parsedPort > 0 && parsedPort <= 65_535
				? parsedPort
				: DEFAULT_VIEWER_PORT,
	};
}

export function boundAttention(items: StatusAttention[]): StatusAttention[] {
	return items.slice(0, MAX_ATTENTION).map((item) => ({
		code: item.code.replace(/[^a-z0-9_]/gi, "_").slice(0, 64),
		severity: item.severity,
		message: item.message.slice(0, 500),
	}));
}

function addDatabaseAttention(state: DatabaseState, attention: StatusAttention[]): void {
	if (state === "missing") {
		attention.push({
			code: "database_missing",
			severity: "error",
			message: "Database does not exist",
		});
	} else if (state === "unavailable") {
		attention.push({
			code: "database_unavailable",
			severity: "error",
			message: "Database could not be read",
		});
	}
}

function addDaemonAttention(state: DaemonState, attention: StatusAttention[]): void {
	if (state === "not_running") {
		attention.push({
			code: "daemon_not_running",
			severity: "warning",
			message: "Daemon is not running; run `codemem serve start` if needed",
		});
	} else if (state === "unavailable") {
		attention.push({
			code: "daemon_unavailable",
			severity: "error",
			message: "Daemon health could not be determined",
		});
	}
}

function doctorOperationalStatus(
	result: Record<string, unknown>,
): OperationalStatusSnapshot | null {
	const diagnostics = result.diagnostics;
	if (!diagnostics || typeof diagnostics !== "object" || Array.isArray(diagnostics)) return null;
	const snapshot = (diagnostics as Record<string, unknown>).operationalStatus;
	if (!snapshot || typeof snapshot !== "object" || Array.isArray(snapshot)) return null;
	return snapshot as OperationalStatusSnapshot;
}

function doctorCapabilitySnapshot(result: Record<string, unknown>): Record<string, unknown> | null {
	const diagnostics = result.diagnostics;
	if (!diagnostics || typeof diagnostics !== "object" || Array.isArray(diagnostics)) return null;
	const capability = (diagnostics as Record<string, unknown>).capability;
	if (!capability || typeof capability !== "object" || Array.isArray(capability)) return null;
	return capability as Record<string, unknown>;
}

function addRuntimeAttention(
	runtime: ViewerRuntimeObservation,
	attention: StatusAttention[],
): void {
	if (runtime.state === "stopped") {
		attention.push({
			code: "viewer_stopped",
			severity: "warning",
			message: "Viewer is stopped; run `codemem serve start` if needed",
		});
		return;
	}
	if (runtime.state === "unreachable") {
		attention.push({
			code: "viewer_unreachable",
			severity: "warning",
			message: "Viewer did not answer its local health check",
		});
		return;
	}
	if (runtime.attention_code) {
		const messages: Record<NonNullable<ViewerRuntimeObservation["attention_code"]>, string> = {
			viewer_pid_malformed: "Viewer PID record is malformed",
			viewer_non_loopback: "Viewer PID record is not loopback; no request was made",
			viewer_not_ready: "Viewer is running but its database is not ready",
			viewer_unexpected_response: "Viewer returned an unexpected local health response",
			viewer_wrong_service: "Local health endpoint did not identify codemem viewer",
		};
		attention.push({
			code: runtime.attention_code,
			severity: "warning",
			message: messages[runtime.attention_code],
		});
	}
}

function boundedCount(value: unknown): number {
	const count = Number(value);
	return Number.isFinite(count) && count > 0 ? Math.trunc(count) : 0;
}

function boundedJobIds(value: unknown): number[] {
	if (!Array.isArray(value)) return [];
	return [
		...new Set(
			value.filter(
				(id): id is number => typeof id === "number" && Number.isSafeInteger(id) && id > 0,
			),
		),
	]
		.toSorted((left, right) => left - right)
		.slice(0, PROCESSING_JOB_CAPACITY);
}

function unknownProcessingJobs(): OperationalProcessingJobs {
	return {
		capacity: PROCESSING_JOB_CAPACITY,
		uncompleted: 0,
		processing: 0,
		failed: 0,
		exhausted: 0,
		pending_grants: 0,
		max_attempt: 0,
		legacy_unrecoverable: 0,
		retry_exhausted_job_ids: [],
		next_action: "upgrade_runtime",
	};
}

function projectProcessingJobs(snapshot: OperationalStatusSnapshot): OperationalProcessingJobs {
	const source = snapshot.processing_jobs;
	if (!source || typeof source !== "object") return unknownProcessingJobs();
	const nextAction: OperationalNextAction = [
		"none",
		"activate_valid_manifest",
		"configure_credential",
		"wait_for_capacity",
		"confirm_retry",
		"restart_daemon",
		"upgrade_runtime",
	].includes(source.next_action as OperationalNextAction)
		? (source.next_action as OperationalNextAction)
		: "upgrade_runtime";
	return {
		capacity: boundedCount(source.capacity) || PROCESSING_JOB_CAPACITY,
		uncompleted: boundedCount(source.uncompleted),
		processing: boundedCount(source.processing),
		failed: boundedCount(source.failed),
		exhausted: boundedCount(source.exhausted),
		pending_grants: boundedCount(source.pending_grants),
		max_attempt: boundedCount(source.max_attempt),
		legacy_unrecoverable: boundedCount(source.legacy_unrecoverable),
		retry_exhausted_job_ids: boundedJobIds(source.retry_exhausted_job_ids),
		next_action: nextAction,
	};
}

function projectDatabaseSubsystems(
	snapshot: OperationalStatusSnapshot | null,
	capability: Record<string, unknown> | null,
	attention: StatusAttention[],
): Pick<
	OperationalStatusReport,
	"maintenance" | "semantic_index" | "raw_events" | "processing_jobs" | "observer"
> {
	if (!snapshot) {
		return {
			maintenance: { state: "unknown" },
			semantic_index: { state: "unknown" },
			raw_events: { state: "unknown", pending: 0, source_gaps: 0 },
			processing_jobs: unknownProcessingJobs(),
			observer: { state: "unknown" },
		};
	}
	if (snapshot.maintenance.state === "failed") {
		attention.push({
			code: "maintenance_failed",
			severity: "error",
			message: "Maintenance has failed jobs; run `codemem maintenance status`",
		});
	} else if (snapshot.maintenance.state === "running") {
		attention.push({
			code: "maintenance_running",
			severity: "warning",
			message: "Maintenance work is in progress",
		});
	}
	if (snapshot.semantic_index.state === "failed") {
		attention.push({
			code: "semantic_index_failed",
			severity: "error",
			message: "Semantic index maintenance failed; run `codemem maintenance status`",
		});
	} else if (snapshot.semantic_index.state === "pending") {
		attention.push({
			code: "semantic_index_pending",
			severity: "warning",
			message: "Semantic index maintenance is pending",
		});
	} else if (snapshot.semantic_index.state === "degraded") {
		attention.push({
			code: "semantic_index_degraded",
			severity: "warning",
			message: "Semantic search is unavailable; keyword search remains available",
		});
	}

	const pending = Math.max(0, Math.trunc(Number(snapshot.raw_events.pending) || 0));
	const sourceGaps = Math.min(
		boundedCount(snapshot.raw_events.source_gaps),
		PROCESSING_JOB_CAPACITY,
	);
	const processingJobs = projectProcessingJobs(snapshot);
	const sourceGapCountAvailable =
		typeof snapshot.raw_events.source_gaps === "number" &&
		Number.isSafeInteger(snapshot.raw_events.source_gaps) &&
		snapshot.raw_events.source_gaps >= 0;
	const rawEventsAvailable = snapshot.raw_events.available === true && sourceGapCountAvailable;
	if (!rawEventsAvailable || sourceGaps > 0) processingJobs.next_action = "upgrade_runtime";
	if (processingJobs.next_action === "upgrade_runtime") {
		processingJobs.retry_exhausted_job_ids = [];
	}
	let rawState: RawEventsState = rawEventsAvailable ? "healthy" : "unknown";
	if (!rawEventsAvailable) {
		attention.push({
			code: "raw_events_unavailable",
			severity: "error",
			message: "Raw-event diagnostics are unavailable; upgrade the runtime",
		});
	}
	if (sourceGaps > 0) {
		if (rawEventsAvailable) rawState = "failing";
		attention.push({
			code: "raw_events_source_gap",
			severity: "error",
			message: `${sourceGaps} raw-event source gap${sourceGaps === 1 ? "" : "s"} detected in bounded scan; upgrade the runtime`,
		});
	}
	if (
		rawEventsAvailable &&
		(processingJobs.exhausted > 0 || boundedCount(snapshot.raw_events.failed_batches) > 0)
	) {
		rawState = "failing";
		if (processingJobs.exhausted === 0) {
			attention.push({
				code: "raw_events_failing",
				severity: "error",
				message: "Raw-event ingestion exhausted retries recently; run `codemem db raw-events-gate`",
			});
		}
	} else if (rawEventsAvailable && sourceGaps === 0 && pending > 0) {
		rawState = "backlogged";
		attention.push({
			code: "raw_events_backlogged",
			severity: "warning",
			message: `${pending} raw events pending; run \`codemem db raw-events-status\``,
		});
	}
	if (processingJobs.legacy_unrecoverable > 0) {
		attention.push({
			code: "processing_jobs_legacy_unrecoverable",
			severity: "error",
			message: "Some raw-event processing ranges are unrecoverable; upgrade the runtime",
		});
	} else if (processingJobs.exhausted > 0) {
		const ids = processingJobs.retry_exhausted_job_ids.join(", ");
		const upgradeRequired = processingJobs.next_action === "upgrade_runtime";
		const activationRequired = processingJobs.next_action === "activate_valid_manifest";
		let message = "Raw-event processing has exhausted automatic retries; confirm a retry";
		if (upgradeRequired) {
			message = "Raw-event processing has exhausted retries; upgrade the runtime before retrying";
		} else if (activationRequired) {
			message =
				"Raw-event processing has exhausted retries; run `codemem setup` to activate a valid manifest before retrying";
		} else if (ids) {
			message = `Raw-event processing exhausted retries for job IDs ${ids}; confirm one exact retry`;
		}
		attention.push({
			code: "processing_jobs_exhausted",
			severity: "error",
			message,
		});
	} else if (processingJobs.failed > 0) {
		attention.push({
			code: "processing_jobs_backoff",
			severity: "warning",
			message: "Raw-event processing has retryable failures and is in backoff",
		});
	} else if (processingJobs.next_action === "wait_for_capacity") {
		attention.push({
			code: "processing_jobs_at_capacity",
			severity: "warning",
			message: "Raw-event processing capacity is full; wait for capacity",
		});
	}

	const configuredObserver = typeof capability?.configurationFingerprint === "string";
	const providerEnabled = capability?.providerEnabled === true;
	let observerState: ObserverState = capability && !configuredObserver ? "unconfigured" : "unknown";
	if (configuredObserver && !providerEnabled) {
		observerState = "pending";
		attention.push({
			code: "observer_pending",
			severity: "warning",
			message: "Observer is pending the privacy, schema, and pack boundaries",
		});
	} else if (configuredObserver) {
		observerState = snapshot.observer.available ? "idle" : "unknown";
	}
	if (configuredObserver && providerEnabled && snapshot.observer.failed_batches > 0) {
		observerState = "failed";
		attention.push({
			code: "observer_failed",
			severity: "error",
			message: "Observer exhausted retries recently; run `codemem db raw-events-gate`",
		});
	} else if (configuredObserver && providerEnabled && snapshot.observer.backoff_batches > 0) {
		observerState = "backoff";
		attention.push({
			code: "observer_backoff",
			severity: "warning",
			message: "Observer has retryable failures; run `codemem db raw-events-status`",
		});
	}
	return {
		maintenance: { state: snapshot.maintenance.state },
		semantic_index: { state: snapshot.semantic_index.state },
		raw_events: { state: rawState, pending, source_gaps: sourceGaps },
		processing_jobs: processingJobs,
		observer: { state: observerState },
	};
}

export async function collectStatusReport(
	opts: StatusOptions,
	deps: StatusDependencies = defaultDependencies,
): Promise<OperationalStatusReport> {
	const checkedAt = deps.now();
	const dataDir = resolveDataDirOpt(opts);
	const health = await deps.requestRpc(dataDir, "GET /v1/health");
	let daemonState: DaemonState = "not_running";
	let databaseState: DatabaseState = "unknown";
	let snapshot: OperationalStatusSnapshot | null = null;
	let capability: Record<string, unknown> | null = null;
	if (health.ok) {
		daemonState = "running";
		capability = doctorCapabilitySnapshot({ diagnostics: health.result });
		const doctor = await deps.requestRpc(dataDir, "GET /v1/doctor");
		if (doctor.ok) {
			databaseState = "ready";
			snapshot = doctorOperationalStatus(doctor.result);
			capability = doctorCapabilitySnapshot(doctor.result);
		} else {
			databaseState = "unavailable";
		}
	} else if (health.error.code !== "daemon_unavailable") {
		daemonState = "unavailable";
		databaseState = "unavailable";
	}

	const pidPath = join(dataDir, "viewer.pid");
	const parsedPid = parseViewerPidRecord(deps.exists(pidPath) ? deps.readText(pidPath) : null);
	const runtime: ViewerRuntimeObservation =
		parsedPid.state === "missing"
			? { state: "stopped" }
			: await observeViewerRuntime(
					parsedPid,
					{
						fetch: deps.fetch,
						isProcessRunning: deps.isProcessRunning,
					},
					viewerDefaultTarget(deps.env),
				);
	const attention: StatusAttention[] = [];
	addDaemonAttention(daemonState, attention);
	addDatabaseAttention(databaseState, attention);
	addRuntimeAttention(runtime, attention);
	if (opts.config) {
		attention.push({
			code: "legacy_config_ignored",
			severity: "warning",
			message:
				"--config no longer selects runtime capability; run `codemem setup` to activate a manifest",
		});
	}
	const subsystems = projectDatabaseSubsystems(snapshot, capability, attention);
	const ok = !attention.some((item) => item.severity === "error");
	const boundedAttention = boundAttention(attention);
	return {
		checked_at: checkedAt.toISOString(),
		ok,
		version: VERSION,
		daemon: { state: daemonState },
		database: { state: databaseState },
		runtime: runtime.pid ? { viewer: runtime.state, pid: runtime.pid } : { viewer: runtime.state },
		...subsystems,
		capability,
		attention: boundedAttention,
	};
}

function stringOrFallback(value: unknown, fallback: string): string {
	return typeof value === "string" ? value : fallback;
}

export function renderStatusReport(report: OperationalStatusReport): string {
	const viewerPidSuffix = report.runtime.pid ? ` (pid ${report.runtime.pid})` : "";
	const sourceGapLabel = report.raw_events.source_gaps === 1 ? "source gap" : "source gaps";
	const rawEventDetails = [
		report.raw_events.pending > 0 ? `${report.raw_events.pending} pending` : null,
		report.raw_events.source_gaps > 0
			? `${report.raw_events.source_gaps} ${sourceGapLabel} (bounded scan)`
			: null,
	].filter((detail): detail is string => detail !== null);
	const rawEventDetailsSuffix =
		rawEventDetails.length > 0 ? ` (${rawEventDetails.join(", ")})` : "";
	const lines = [
		`codemem status ${report.ok ? "OK" : "ATTENTION"}`,
		`Daemon:         ${report.daemon.state}`,
		`Database:       ${report.database.state}`,
		`Viewer:         ${report.runtime.viewer}${viewerPidSuffix}`,
		`Maintenance:    ${report.maintenance.state}`,
		`Semantic index: ${report.semantic_index.state}`,
		`Raw events:     ${report.raw_events.state}${rawEventDetailsSuffix}`,
		`Processing jobs: ${report.processing_jobs.uncompleted}/${report.processing_jobs.capacity}` +
			` (processing ${report.processing_jobs.processing}, exhausted ${report.processing_jobs.exhausted},` +
			` next ${report.processing_jobs.next_action})`,
		...(report.processing_jobs.retry_exhausted_job_ids.length > 0
			? [`Retry job IDs:  ${report.processing_jobs.retry_exhausted_job_ids.join(", ")}`]
			: []),
		`Observer:       ${report.observer.state}`,
	];
	if (report.capability) {
		const provider = report.capability.summaryProvider;
		const providerFingerprint =
			provider && typeof provider === "object" && !Array.isArray(provider)
				? (provider as Record<string, unknown>).providerFingerprint
				: null;
		lines.push(
			`Capability:     ${stringOrFallback(
				report.capability.runtimeReason,
				stringOrFallback(report.capability.mode, "unknown"),
			)}`,
			`Manifest:       ${stringOrFallback(report.capability.configurationFingerprint, "none")}`,
			`Provider:       ${stringOrFallback(
				providerFingerprint,
				stringOrFallback(report.capability.providerFingerprint, "none"),
			)}`,
			`Provider health: ${stringOrFallback(report.capability.providerHealth, "unknown")}`,
			`Schema:         ${stringOrFallback(report.capability.schemaReadiness, "unknown")}`,
			`Pack:           ${stringOrFallback(report.capability.packReadiness, "unknown")}`,
		);
	}
	if (report.attention.length > 0) {
		lines.push(
			"Attention:",
			...report.attention.map((item) => `- ${item.severity}: ${item.message}`),
		);
	}
	return lines.join("\n");
}

export function createStatusCommand(overrides: Partial<StatusDependencies> = {}): Command {
	const deps: StatusDependencies = { ...defaultDependencies, ...overrides };
	const command = new Command("status")
		.configureHelp(helpStyle)
		.description("Show local operational status")
		.allowUnknownOption(true)
		.allowExcessArguments(true);
	addDbOption(command);
	addConfigOption(command);
	addJsonOption(command);
	return command.action(async (opts: StatusOptions, actionCommand: Command) => {
		if (actionCommand.args.length > 0) {
			const message = "status accepts only documented options and no positional arguments";
			if (opts.json) deps.writeStdout(JSON.stringify({ error: "usage_error", message }));
			else deps.writeStderr(message);
			deps.setExitCode(2);
			return;
		}
		try {
			const report = await collectStatusReport(opts, deps);
			deps.writeStdout(opts.json ? JSON.stringify(report) : renderStatusReport(report));
			deps.setExitCode(0);
		} catch {
			const error = { error: "status_failed", message: "Unable to collect operational status" };
			if (opts.json) deps.writeStdout(JSON.stringify(error));
			else deps.writeStderr(error.message);
			deps.setExitCode(1);
		}
	});
}

export const statusCommand = createStatusCommand();
