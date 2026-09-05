import { randomUUID } from "node:crypto";
import * as p from "@clack/prompts";
import { resolveProject } from "@codemem/core";
import { createMcpRpcClient } from "@codemem/mcp";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import {
	addDbOption,
	addJsonOption,
	type DbOpts,
	emitJsonError,
	type JsonOpts,
	resolveDataDirOpt,
} from "../shared-options.js";
import { type DaemonJobRunOutcome, runDaemonJob } from "./daemon-job.js";

function emitJobError(
	json: boolean | undefined,
	outcome: Extract<DaemonJobRunOutcome, { ok: false }>,
): void {
	const message = outcome.jobId
		? `${outcome.error.message} Job ID: ${outcome.jobId}`
		: outcome.error.message;
	if (json) emitJsonError(outcome.error.code, message);
	else {
		p.log.error(message);
		process.exitCode = 1;
	}
}

function formatBytes(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function parseOptionalPositiveInt(value: string | undefined): number | undefined {
	if (!value) return undefined;
	const parsed = Number.parseInt(value, 10);
	if (!Number.isFinite(parsed) || parsed <= 0) {
		throw new Error(`invalid positive integer: ${value}`);
	}
	return parsed;
}

function parseKindsCsv(value: string | undefined): string[] | undefined {
	if (!value) return undefined;
	const kinds = value
		.split(",")
		.map((kind) => kind.trim())
		.filter((kind) => kind.length > 0);
	return kinds.length > 0 ? kinds : undefined;
}

export const dbCommand = new Command("db")
	.configureHelp(helpStyle)
	.description("Database maintenance");

// --- db init ---
const initCmd = new Command("init")
	.configureHelp(helpStyle)
	.description("Create or verify the SQLite database and schema");
addDbOption(initCmd);
initCmd.action(async (opts: DbOpts) => {
	const outcome = await runDaemonJob(opts, "db.init", {});
	if (!outcome.ok) {
		emitJobError(undefined, outcome);
		return;
	}
	const result = outcome.result as { path: string; sizeBytes: number };
	p.intro("codemem db init");
	p.log.success(`Database ready: ${result.path}`);
	p.outro(`Size: ${result.sizeBytes.toLocaleString()} bytes`);
});
dbCommand.addCommand(initCmd);

// --- db vacuum ---
const vacuumCmd = new Command("vacuum")
	.configureHelp(helpStyle)
	.description("Run VACUUM on the SQLite database");
addDbOption(vacuumCmd);
vacuumCmd.action(async (opts: DbOpts) => {
	const outcome = await runDaemonJob(opts, "db.vacuum", {});
	if (!outcome.ok) {
		emitJobError(undefined, outcome);
		return;
	}
	const result = outcome.result as { path: string; sizeBytes: number };
	p.intro("codemem db vacuum");
	p.log.success(`Vacuumed: ${result.path}`);
	p.outro(`Size: ${result.sizeBytes.toLocaleString()} bytes`);
});
dbCommand.addCommand(vacuumCmd);

// --- db prune-raw-events ---
const pruneRawEventsCmd = new Command("prune-raw-events")
	.configureHelp(helpStyle)
	.description("Delete raw_events older than a cutoff (age-based), with dry-run and VACUUM support")
	.option("--dry-run", "show current size/targets without deleting")
	.option("--max-age-days <days>", "retention age threshold in days", "90")
	.option("--vacuum", "run VACUUM explicitly after prune completes");
addDbOption(pruneRawEventsCmd);
pruneRawEventsCmd.action(
	async (
		opts: DbOpts & {
			dryRun?: boolean;
			maxAgeDays: string;
			vacuum?: boolean;
		},
	) => {
		const rawAge = opts.maxAgeDays.trim();
		const maxAgeDays = Number.parseInt(rawAge, 10);
		if (!/^\d+$/.test(rawAge) || !Number.isInteger(maxAgeDays) || maxAgeDays < 1) {
			p.log.error(
				`--max-age-days must be a positive integer (got "${opts.maxAgeDays}"). ` +
					"Refusing to run a destructive prune on invalid input.",
			);
			process.exitCode = 1;
			return;
		}
		const outcome = await runDaemonJob(
			opts,
			"raw-events.prune",
			{ maxAgeDays, vacuum: opts.vacuum === true },
			opts.dryRun === true,
		);
		if (!outcome.ok) {
			emitJobError(undefined, outcome);
			return;
		}
		const result = outcome.result as {
			status: { total_rows: number; estimated_bytes: number; candidate_rows: number };
			deleted: number;
			vacuumed: { sizeBytes: number } | null;
		};
		p.intro("codemem db prune-raw-events");
		p.log.warn(
			"raw_events is the re-extraction source. Age-based purge is NOT extraction-aware: " +
				"pruning a window shorter than your extraction lag can delete un-extracted events. " +
				"The 90-day default mitigates this.",
		);
		p.log.info(
			`raw_events: ${result.status.total_rows.toLocaleString()} rows, ~${formatBytes(result.status.estimated_bytes)} on disk`,
		);
		p.log.info(
			`Older than ${maxAgeDays} day(s): ${result.status.candidate_rows.toLocaleString()} row(s) to delete`,
		);
		if (opts.dryRun) {
			p.outro("Dry run only; no changes made");
			return;
		}
		p.log.info(`Deleted raw_events: ${result.deleted.toLocaleString()}`);
		if (result.vacuumed) {
			p.outro(`Done. VACUUM complete. File size is now ${formatBytes(result.vacuumed.sizeBytes)}.`);
			return;
		}
		p.outro(
			"Done. SQLite file size may not shrink until you run `codemem db vacuum` explicitly (or re-run this command with --vacuum).",
		);
	},
);
dbCommand.addCommand(pruneRawEventsCmd);

// --- db raw-events-status ---
const rawEventsStatusCmd = new Command("raw-events-status")
	.configureHelp(helpStyle)
	.description("Show pending raw-event backlog by source stream")
	.option("-n, --limit <n>", "max rows to show", "25");
addDbOption(rawEventsStatusCmd);
addJsonOption(rawEventsStatusCmd);
rawEventsStatusCmd.action(async (opts: DbOpts & JsonOpts & { limit: string }) => {
	const outcome = await runDaemonJob(
		opts,
		"report.raw-events",
		{
			limit: Number.parseInt(opts.limit, 10) || 25,
		},
		true,
	);
	if (!outcome.ok) {
		emitJobError(opts.json, outcome);
		return;
	}
	const result = outcome.result as {
		totals: { pending: number; sessions: number };
		items: Array<{
			source: string;
			stream_id: string;
			last_received_event_seq: number;
			last_flushed_event_seq: number;
			project: string | null;
		}>;
	};
	if (opts.json) {
		console.log(JSON.stringify(result, null, 2));
		return;
	}
	p.intro("codemem db raw-events-status");
	p.log.info(
		`Totals: ${result.totals.pending.toLocaleString()} pending across ${result.totals.sessions.toLocaleString()} session(s)`,
	);
	if (result.items.length === 0) {
		p.outro("No pending raw events");
		return;
	}
	for (const item of result.items) {
		p.log.message(
			`${item.source}:${item.stream_id} pending=${Math.max(0, item.last_received_event_seq - item.last_flushed_event_seq)} ` +
				`received=${item.last_received_event_seq} flushed=${item.last_flushed_event_seq} project=${item.project ?? ""}`,
		);
	}
	p.outro("done");
});
dbCommand.addCommand(rawEventsStatusCmd);

// --- db raw-events-retry ---
const rawEventsRetryCmd = new Command("raw-events-retry")
	.configureHelp(helpStyle)
	.description("Requeue failed raw-event flush batches")
	.option("-n, --limit <n>", "max failed batches to requeue", "25");
addDbOption(rawEventsRetryCmd);
rawEventsRetryCmd.action(async (opts: DbOpts & { limit: string }) => {
	const outcome = await runDaemonJob(opts, "raw-events.retry", {
		limit: Number.parseInt(opts.limit, 10) || 25,
	});
	if (!outcome.ok) {
		emitJobError(undefined, outcome);
		return;
	}
	const result = outcome.result as { retried: number };
	p.intro("codemem db raw-events-retry");
	p.outro(`Requeued ${result.retried.toLocaleString()} failed batch(es)`);
});
dbCommand.addCommand(rawEventsRetryCmd);

// --- db raw-events-doctor-retry ---
const rawEventsDoctorRetryCmd = new Command("raw-events-doctor-retry")
	.configureHelp(helpStyle)
	.description("Confirm one displayed retry-exhausted raw-event processing job")
	.argument("<job-id>", "displayed processing job ID");
addDbOption(rawEventsDoctorRetryCmd);
rawEventsDoctorRetryCmd.action(async (jobId: string, opts: DbOpts) => {
	const client = createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) });
	const displayed = await client.request("GET /v1/processing-jobs/:id", { id: jobId });
	if (!displayed.ok) {
		p.log.error(displayed.error.message);
		process.exitCode = 1;
		return;
	}
	const job = displayed.result.job;
	if (!job || typeof job !== "object" || Array.isArray(job)) {
		p.log.error("Processing job was not found.");
		process.exitCode = 1;
		return;
	}
	const value = job as Record<string, unknown>;
	const attempt = value.attempt as Record<string, unknown> | null;
	const retryTarget = value.retryTarget as Record<string, unknown> | null;
	if (!attempt || typeof attempt !== "object" || !retryTarget || typeof retryTarget !== "object") {
		p.log.error("Processing job snapshot is invalid.");
		process.exitCode = 1;
		return;
	}
	const attemptProviderFingerprint = attempt.providerFingerprint;
	const attemptManifestFingerprint = attempt.manifestFingerprint;
	const knownAttempt =
		typeof attemptProviderFingerprint === "string" &&
		typeof attemptManifestFingerprint === "string";
	const legacyUnknownAttempt =
		attemptProviderFingerprint === null && attemptManifestFingerprint === null;
	if (
		value.component !== "summary" ||
		value.state !== "retry_exhausted" ||
		(!knownAttempt && !legacyUnknownAttempt) ||
		typeof retryTarget.providerFingerprint !== "string" ||
		typeof retryTarget.manifestFingerprint !== "string" ||
		typeof attempt.count !== "number" ||
		typeof attempt.claimGeneration !== "number"
	) {
		p.log.error("Processing job snapshot is not retryable.");
		process.exitCode = 1;
		return;
	}
	p.log.info(
		`Job ${String(value.jobId)}: ${String(value.component)} ${String(value.state)} ` +
			`attempt=${String(attempt.count)} claim=${String(attempt.claimGeneration)} ` +
			`manifest=${attemptManifestFingerprint ?? "legacy_unknown"} ` +
			`provider=${attemptProviderFingerprint ?? "legacy_unknown"} ` +
			`retry_manifest=${String(retryTarget.manifestFingerprint)} ` +
			`retry_provider=${String(retryTarget.providerFingerprint)}`,
	);
	let confirmed: Awaited<ReturnType<typeof p.confirm>>;
	try {
		confirmed = await p.confirm({
			message: "Create one retry grant for this displayed job?",
			initialValue: false,
		});
	} catch {
		p.log.error("Retry confirmation failed.");
		process.exitCode = 1;
		return;
	}
	if (!confirmed || p.isCancel(confirmed)) {
		process.exitCode = 1;
		return;
	}
	const retried = await client.request("POST /v1/processing-jobs/:id/doctor-retry", {
		id: jobId,
		producerReceiptId: randomUUID(),
		expectedRole: "summary",
		expectedProviderFingerprint: attemptProviderFingerprint,
		expectedManifestFingerprint: attemptManifestFingerprint,
		expectedAttemptCount: attempt.count,
		expectedClaimGeneration: attempt.claimGeneration,
	});
	if (!retried.ok) {
		p.log.error(retried.error.message);
		process.exitCode = 1;
		return;
	}
	p.log.success(`Retry grant ${String(retried.result.grantState)} for job ${jobId}.`);
});
dbCommand.addCommand(rawEventsDoctorRetryCmd);

// --- db raw-events-gate ---
const rawEventsGateCmd = new Command("raw-events-gate")
	.configureHelp(helpStyle)
	.description("Validate raw-event reliability thresholds (non-zero exit on failure)")
	.option("--min-flush-success-rate <rate>", "minimum flush success rate", "0.95")
	.option("--max-dropped-event-rate <rate>", "maximum dropped event rate", "0.05")
	.option("--min-session-boundary-accuracy <rate>", "minimum session boundary accuracy", "0.9")
	.option("--window-hours <hours>", "lookback window in hours", "24");
addDbOption(rawEventsGateCmd);
addJsonOption(rawEventsGateCmd);
rawEventsGateCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				minFlushSuccessRate: string;
				maxDroppedEventRate: string;
				minSessionBoundaryAccuracy: string;
				windowHours: string;
			},
	) => {
		const outcome = await runDaemonJob(
			opts,
			"gate.raw-events",
			{
				minFlushSuccessRate: Number.parseFloat(opts.minFlushSuccessRate),
				maxDroppedEventRate: Number.parseFloat(opts.maxDroppedEventRate),
				minSessionBoundaryAccuracy: Number.parseFloat(opts.minSessionBoundaryAccuracy),
				windowHours: Number.parseFloat(opts.windowHours),
			},
			true,
		);
		if (!outcome.ok) {
			emitJobError(opts.json, outcome);
			return;
		}
		const result = outcome.result as {
			passed: boolean;
			failures: string[];
			metrics: {
				rates: {
					flush_success_rate: number;
					dropped_event_rate: number;
					session_boundary_accuracy: number;
				};
				window_hours: number | null;
			};
		};

		if (opts.json) {
			console.log(JSON.stringify(result, null, 2));
			if (!result.passed) process.exitCode = 1;
			return;
		}

		p.intro("codemem db raw-events-gate");
		p.log.info(
			[
				`flush_success_rate:          ${result.metrics.rates.flush_success_rate.toFixed(4)}`,
				`dropped_event_rate:          ${result.metrics.rates.dropped_event_rate.toFixed(4)}`,
				`session_boundary_accuracy:   ${result.metrics.rates.session_boundary_accuracy.toFixed(4)}`,
				`window_hours:                ${result.metrics.window_hours ?? "all"}`,
			].join("\n"),
		);

		if (result.passed) {
			p.outro("reliability gate passed");
		} else {
			for (const f of result.failures) {
				p.log.error(f);
			}
			p.outro("reliability gate FAILED");
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(rawEventsGateCmd);

// --- db rename-project ---
const renameProjectCmd = new Command("rename-project")
	.configureHelp(helpStyle)
	.description("Rename a project across sessions and related tables")
	.argument("<old-name>", "current project name")
	.argument("<new-name>", "new project name")
	.option("--apply", "apply changes (default is dry-run)");
addDbOption(renameProjectCmd);
renameProjectCmd.action(
	async (oldName: string, newName: string, opts: DbOpts & { apply?: boolean }) => {
		const dryRun = !opts.apply;
		const outcome = await runDaemonJob(opts, "projects.rename", { oldName, newName }, dryRun);
		if (!outcome.ok) {
			emitJobError(undefined, outcome);
			return;
		}
		const result = outcome.result as { counts: Record<string, number> };
		const action = dryRun ? "Will rename" : "Renamed";
		p.intro("codemem db rename-project");
		p.log.info(`${action} ${oldName} → ${newName}`);
		p.log.info(
			[
				`Sessions: ${result.counts.sessions}`,
				`Raw event sessions: ${result.counts.raw_event_sessions}`,
			].join("\n"),
		);
		p.outro(dryRun ? "Pass --apply to execute" : "done");
	},
);
dbCommand.addCommand(renameProjectCmd);

// --- db normalize-projects ---
const normalizeProjectsCmd = new Command("normalize-projects")
	.configureHelp(helpStyle)
	.description("Normalize path-like project identifiers to their basename")
	.option("--apply", "apply changes (default is dry-run)");
addDbOption(normalizeProjectsCmd);
normalizeProjectsCmd.action(async (opts: DbOpts & { apply?: boolean }) => {
	const dryRun = !opts.apply;
	const outcome = await runDaemonJob(opts, "projects.normalize", {}, dryRun);
	if (!outcome.ok) {
		emitJobError(undefined, outcome);
		return;
	}
	const result = outcome.result as {
		counts: Record<string, number>;
		rewrites: Array<{ from: string; to: string }>;
	};
	p.intro("codemem db normalize-projects");
	p.log.info(`Dry run: ${dryRun}`);
	p.log.info(
		[
			`Sessions to update: ${result.counts.sessions}`,
			`Raw event sessions to update: ${result.counts.raw_event_sessions}`,
		].join("\n"),
	);
	if (result.rewrites.length > 0) {
		p.log.info("Rewritten paths:");
		for (const { from, to } of result.rewrites) p.log.message(`  ${from} → ${to}`);
	}
	p.outro(dryRun ? "Pass --apply to execute" : "done");
});
dbCommand.addCommand(normalizeProjectsCmd);

// --- db size-report ---
const sizeReportCmd = new Command("size-report")
	.configureHelp(helpStyle)
	.description("Show SQLite file size and major storage consumers")
	.option("--limit <n>", "number of largest tables/indexes to show", "12");
addDbOption(sizeReportCmd);
addJsonOption(sizeReportCmd);
sizeReportCmd.action(async (opts: DbOpts & JsonOpts & { limit: string }) => {
	const limit = Math.max(1, Number.parseInt(opts.limit, 10) || 12);
	const outcome = await runDaemonJob(opts, "report.db-size", { limit }, true);
	if (!outcome.ok) {
		emitJobError(opts.json, outcome);
		return;
	}
	const result = outcome.result as {
		file_size_bytes: number;
		db_size_bytes: number;
		free_bytes: number;
		tables: Array<{ name: string; size_bytes: number }>;
	};

	if (opts.json) {
		console.log(JSON.stringify(result, null, 2));
		return;
	}

	p.intro("codemem db size-report");
	p.log.info(
		[
			`File size:     ${formatBytes(result.file_size_bytes)}`,
			`DB size:       ${formatBytes(result.db_size_bytes)}`,
			`Free space:    ${formatBytes(result.free_bytes)}`,
		].join("\n"),
	);
	if (result.tables.length > 0) {
		p.log.info("Largest objects:");
		for (const t of result.tables) {
			p.log.message(`  ${t.name.padEnd(40)} ${formatBytes(t.size_bytes).padStart(10)}`);
		}
	}
	p.outro("done");
});
dbCommand.addCommand(sizeReportCmd);

// --- db backfill-tags ---
const backfillTagsCmd = new Command("backfill-tags")
	.configureHelp(helpStyle)
	.description("Populate tags_text for memories where tags are empty")
	.option("--limit <n>", "max memories to check")
	.option("--since <iso>", "only memories created at/after this ISO timestamp")
	.option("--project <project>", "project identifier (defaults to git repo root)")
	.option("--all-projects", "backfill across all projects")
	.option("--inactive", "include inactive memories")
	.option("--dry-run", "preview updates without writing");
addDbOption(backfillTagsCmd);
addJsonOption(backfillTagsCmd);
backfillTagsCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit?: string;
				since?: string;
				project?: string;
				allProjects?: boolean;
				inactive?: boolean;
				dryRun?: boolean;
			},
	) => {
		try {
			const limit = parseOptionalPositiveInt(opts.limit);
			const project =
				opts.allProjects === true
					? null
					: opts.project?.trim() ||
						process.env.CODEMEM_PROJECT?.trim() ||
						resolveProject(process.cwd(), null);
			const outcome = await runDaemonJob(
				opts,
				"tags.backfill",
				{
					limit,
					since: opts.since ?? null,
					project,
					activeOnly: !opts.inactive,
				},
				opts.dryRun === true,
			);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as { checked: number; updated: number; skipped: number };

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would update" : "Updated";
			p.intro("codemem db backfill-tags");
			p.log.success(`${action} ${result.updated} memories (skipped ${result.skipped})`);
			p.outro(`Checked ${result.checked} memories`);
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(backfillTagsCmd);

// --- db prune-observations ---
const pruneObsCmd = new Command("prune-observations")
	.configureHelp(helpStyle)
	.description("Deactivate low-signal observations (does not delete rows)")
	.option("--limit <n>", "max observations to check")
	.option("--dry-run", "preview deactivations without writing");
addDbOption(pruneObsCmd);
addJsonOption(pruneObsCmd);
pruneObsCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit?: string;
				dryRun?: boolean;
			},
	) => {
		try {
			const limit = parseOptionalPositiveInt(opts.limit);
			const outcome = await runDaemonJob(
				opts,
				"observations.prune",
				{ limit },
				opts.dryRun === true,
			);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as { checked: number; deactivated: number };

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would deactivate" : "Deactivated";
			p.intro("codemem db prune-observations");
			p.outro(`${action} ${result.deactivated} of ${result.checked} observations`);
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(pruneObsCmd);

// --- db prune-memories ---
const pruneMemCmd = new Command("prune-memories")
	.configureHelp(helpStyle)
	.description("Deactivate low-signal memories across selected kinds")
	.option("--limit <n>", "max memories to check")
	.option("--kinds <csv>", "comma-separated memory kinds (default set when omitted)")
	.option("--dry-run", "preview deactivations without writing");
addDbOption(pruneMemCmd);
addJsonOption(pruneMemCmd);
pruneMemCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit?: string;
				kinds?: string;
				dryRun?: boolean;
			},
	) => {
		try {
			const limit = parseOptionalPositiveInt(opts.limit);
			const kinds = parseKindsCsv(opts.kinds);
			const outcome = await runDaemonJob(
				opts,
				"memories.prune",
				{ kinds, limit },
				opts.dryRun === true,
			);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as { checked: number; deactivated: number };

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would deactivate" : "Deactivated";
			p.intro("codemem db prune-memories");
			p.outro(`${action} ${result.deactivated} of ${result.checked} memories`);
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(pruneMemCmd);

// --- db dedup-memories ---
const dedupCmd = new Command("dedup-memories")
	.configureHelp(helpStyle)
	.description(
		"Deactivate near-duplicate memories (cross-session, same normalized title within time window)",
	)
	.option("--window <ms>", "max time gap in milliseconds between duplicates (default: 3600000)")
	.option("--limit <n>", "max pairs to check")
	.option("--dry-run", "preview deactivations without writing");
addDbOption(dedupCmd);
addJsonOption(dedupCmd);
dedupCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				window?: string;
				limit?: string;
				dryRun?: boolean;
			},
	) => {
		try {
			const windowMs = parseOptionalPositiveInt(opts.window);
			const limit = parseOptionalPositiveInt(opts.limit);
			const outcome = await runDaemonJob(
				opts,
				"memories.dedup",
				{ windowMs, limit },
				opts.dryRun === true,
			);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as {
				checked: number;
				deactivated: number;
				pairs: Array<{ kept_id: number; deactivated_id: number; title: string }>;
			};

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would deactivate" : "Deactivated";
			p.intro("codemem db dedup-memories");
			if (result.pairs.length > 0 && result.pairs.length <= 20) {
				for (const pair of result.pairs) {
					p.log.info(
						`${action} [${pair.deactivated_id}] (kept [${pair.kept_id}]): ${pair.title.slice(0, 80)}`,
					);
				}
			}
			p.outro(`${action} ${result.deactivated} duplicates from ${result.checked} pairs`);
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(dedupCmd);

const backfillDedupKeysCmd = new Command("backfill-dedup-keys")
	.configureHelp(helpStyle)
	.description("Populate missing memory_items.dedup_key values for legacy rows")
	.option("--limit <n>", "max memories to check")
	.option("--dry-run", "preview updates without writing");
addDbOption(backfillDedupKeysCmd);
addJsonOption(backfillDedupKeysCmd);
backfillDedupKeysCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit?: string;
				dryRun?: boolean;
			},
	) => {
		try {
			const limit = parseOptionalPositiveInt(opts.limit);
			const outcome = await runDaemonJob(
				opts,
				"dedup-keys.backfill",
				{ limit },
				opts.dryRun === true,
			);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as { checked: number; updated: number; skipped: number };

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would update" : "Updated";
			p.intro("codemem db backfill-dedup-keys");
			p.log.success(`${action} ${result.updated} memories (skipped ${result.skipped})`);
			p.outro(`Checked ${result.checked} memories`);
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(backfillDedupKeysCmd);

// --- db backfill-narrative ---
const backfillNarrativeCmd = new Command("backfill-narrative")
	.configureHelp(helpStyle)
	.description(
		"Extract narrative from session_summary body_text (## Completed / ## Learned sections)",
	)
	.option("--limit <n>", "max memories to check")
	.option("--dry-run", "preview updates without writing");
addDbOption(backfillNarrativeCmd);
addJsonOption(backfillNarrativeCmd);
backfillNarrativeCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit?: string;
				dryRun?: boolean;
			},
	) => {
		try {
			const limit = parseOptionalPositiveInt(opts.limit);
			const outcome = await runDaemonJob(
				opts,
				"narrative.backfill",
				{ limit },
				opts.dryRun === true,
			);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as { checked: number; updated: number; skipped: number };

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would update" : "Updated";
			p.intro("codemem db backfill-narrative");
			p.log.success(`${action} ${result.updated} memories (skipped ${result.skipped})`);
			p.outro(`Checked ${result.checked} memories`);
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(backfillNarrativeCmd);

// --- db ai-backfill-structured ---
const aiBackfillStructuredCmd = new Command("ai-backfill-structured")
	.configureHelp(helpStyle)
	.description(
		"Use GPT-5.4 to populate missing narrative/facts/concepts for older non-session-summary memories",
	)
	.option("--limit <n>", "max memories to check")
	.option("--kinds <csv>", "comma-separated kinds to target")
	.option(
		"--overwrite",
		"overwrite existing structured fields instead of only filling missing ones",
	)
	.option("--dry-run", "preview updates without writing");
addDbOption(aiBackfillStructuredCmd);
addJsonOption(aiBackfillStructuredCmd);
aiBackfillStructuredCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit?: string;
				kinds?: string;
				overwrite?: boolean;
				dryRun?: boolean;
			},
	) => {
		try {
			const limit = parseOptionalPositiveInt(opts.limit);
			const kinds = parseKindsCsv(opts.kinds);
			const outcome = await runDaemonJob(
				opts,
				"structured.backfill",
				{
					limit,
					kinds,
					overwrite: opts.overwrite === true,
				},
				opts.dryRun === true,
			);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as {
				checked: number;
				updated: number;
				skipped: number;
				failed: number;
			};

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would update" : "Updated";
			p.intro("codemem db ai-backfill-structured");
			p.log.success(
				`${action} ${result.updated} memories (skipped ${result.skipped}, failed ${result.failed})`,
			);
			p.outro(`Checked ${result.checked} memories`);
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(aiBackfillStructuredCmd);

// --- db scan-secrets ---
const scanSecretsCmd = new Command("scan-secrets")
	.configureHelp(helpStyle)
	.description("Sweep existing memories and redact any secrets found in stored content")
	.option("--limit <n>", "max memories to scan in this run")
	.option("--dry-run", "report detections without rewriting any rows");
addDbOption(scanSecretsCmd);
addJsonOption(scanSecretsCmd);
scanSecretsCmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit?: string;
				dryRun?: boolean;
			},
	) => {
		try {
			const limit = parseOptionalPositiveInt(opts.limit);
			const outcome = await runDaemonJob(opts, "secrets.scan", { limit }, opts.dryRun === true);
			if (!outcome.ok) {
				emitJobError(opts.json, outcome);
				return;
			}
			const result = outcome.result as {
				checked: number;
				updated: number;
				skippedOversized: number;
				detections: Array<{ kind: string; count: number }>;
				samples: Array<{
					id: number;
					redactedTitle?: string | null;
					detections: Array<{ kind: string; count: number }>;
				}>;
			};

			if (opts.json) {
				console.log(JSON.stringify(result, null, 2));
				return;
			}

			const action = opts.dryRun ? "Would redact" : "Redacted";
			p.intro("codemem db scan-secrets");
			p.log.success(`${action} ${result.updated} of ${result.checked} memories`);
			if (result.skippedOversized > 0) {
				p.log.warn(`Skipped ${result.skippedOversized} oversized rows (above default 1 MiB cap)`);
			}
			if (result.detections.length > 0) {
				const summary = result.detections.map((d) => `${d.kind}=${d.count}`).join(", ");
				p.log.info(`Detections: ${summary}`);
			}
			if (result.samples.length > 0) {
				const lines = result.samples.map((s) => {
					const kinds = s.detections.map((d) => `${d.kind}=${d.count}`).join(", ");
					const title = (s.redactedTitle ?? "").slice(0, 80);
					return `  #${s.id} [${kinds}]  ${title}`;
				});
				p.log.info(`Affected memories:\n${lines.join("\n")}`);
			}
			p.outro("Re-run with no changes to confirm idempotency");
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 1;
		}
	},
);
dbCommand.addCommand(scanSecretsCmd);
