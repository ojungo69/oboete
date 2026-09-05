/* Health overview renderer — reads system, usage, and raw-event
 * signals off the global state, computes a weighted risk score + set
 * of drivers, then renders the cards row and recommended-action list.
 * The risk scoring and recommendation rules are the single source of
 * truth for the Health tab's "Overall health" status. */

import {
	formatAgeShort,
	formatReductionPercent,
	parsePercentValue,
	secondsSince,
} from "../../../lib/format";
import { state } from "../../../lib/state";
import { buildHealthCard, renderActionList, renderHealthCards } from "../components";
import type { HealthAction, HealthCardInput } from "../types";

export function renderHealthOverview() {
	const healthGrid = document.getElementById("healthGrid");
	const healthMeta = document.getElementById("healthMeta");
	const healthActions = document.getElementById("healthActions");
	const healthDot = document.getElementById("healthDot");
	if (!healthGrid || !healthMeta) return;

	const stats = state.lastStatsPayload || {};
	const usagePayload = state.lastUsagePayload || {};
	const raw =
		state.lastRawEventsPayload && typeof state.lastRawEventsPayload === "object"
			? state.lastRawEventsPayload
			: {};
	const maintenanceJobs: Array<{
		kind?: string;
		title?: string;
		status?: string;
		message?: string | null;
		error?: string | null;
		progress?: { current?: number; total?: number | null; unit?: string };
	}> = Array.isArray(stats.maintenance_jobs) ? stats.maintenance_jobs : [];
	const reliability = stats.reliability || {};
	const counts = reliability.counts || {};
	const rates = reliability.rates || {};
	const dbStats = stats.database || {};
	const totals =
		usagePayload.totals_filtered ||
		usagePayload.totals ||
		usagePayload.totals_global ||
		stats.usage?.totals ||
		{};
	const recentPacks = Array.isArray(usagePayload.recent_packs) ? usagePayload.recent_packs : [];
	const lastPackAt = recentPacks.length ? recentPacks[0]?.created_at : null;
	const latestPackMeta = recentPacks.length ? recentPacks[0]?.metadata_json || {} : {};
	const latestPackDeduped = Number(latestPackMeta?.exact_duplicates_collapsed || 0);
	const rawPending = Number(raw.pending || 0);
	const erroredBatches = Number(counts.errored_batches || 0);
	const flushSuccessRate = Number(rates.flush_success_rate ?? 1);
	const droppedRate = Number(rates.dropped_event_rate || 0);
	const reductionLabel = formatReductionPercent(totals.tokens_saved, totals.tokens_read);
	const reductionPercent = parsePercentValue(reductionLabel);
	const tagCoverage = Number(dbStats.tags_coverage || 0);
	const packAgeSeconds = secondsSince(lastPackAt);
	const hasBacklog = rawPending >= 200;

	// Risk scoring
	let riskScore = 0;
	const drivers: string[] = [];
	if (rawPending >= 1000) {
		riskScore += 40;
		drivers.push("high raw-event backlog");
	} else if (rawPending >= 200) {
		riskScore += 24;
		drivers.push("growing raw-event backlog");
	}
	if (erroredBatches > 0 && rawPending >= 200) {
		riskScore += erroredBatches >= 5 ? 10 : 6;
		drivers.push("batch errors during backlog pressure");
	}
	if (flushSuccessRate < 0.95) {
		riskScore += 20;
		drivers.push("lower flush success");
	}
	if (droppedRate > 0.02) {
		riskScore += 24;
		drivers.push("high dropped-event rate");
	} else if (droppedRate > 0.005) {
		riskScore += 10;
		drivers.push("non-trivial dropped-event rate");
	}
	if (reductionPercent !== null && reductionPercent < 10) {
		riskScore += 8;
		drivers.push("low retrieval reduction");
	}
	if (packAgeSeconds !== null && packAgeSeconds > 86400) {
		riskScore += 12;
		drivers.push("memory pack activity is old");
	}

	let statusLabel = "Healthy";
	let statusClass = "status-healthy";
	if (riskScore >= 60) {
		statusLabel = "Attention";
		statusClass = "status-attention";
	} else if (riskScore >= 25) {
		statusLabel = "Degraded";
		statusClass = "status-degraded";
	}
	// Update header health dot
	if (healthDot) {
		healthDot.className = `health-dot ${statusClass}`;
		healthDot.title = statusLabel;
	}

	const retrievalDetail = `${Number(totals.tokens_saved || 0).toLocaleString()} saved tokens · ${latestPackDeduped.toLocaleString()} deduped in latest pack`;
	const pipelineDetail = rawPending > 0 ? "Queue is actively draining" : "Queue is clear";
	const freshnessDetail = `last pack ${formatAgeShort(packAgeSeconds)} ago`;

	// Build maintenance card(s) for active background jobs
	const maintenanceCards: HealthCardInput[] = maintenanceJobs.map((job) => {
		const current = Number(job.progress?.current || 0);
		const total = typeof job.progress?.total === "number" ? job.progress.total : null;
		const unit = String(job.progress?.unit || "items");
		const pct = total && total > 0 ? ` (${Math.round((100 * current) / total)}%)` : "";
		const progress =
			total && total > 0
				? `${current.toLocaleString()}/${total.toLocaleString()} ${unit}${pct}`
				: `${current.toLocaleString()} ${unit}`;
		const isFailed = job.status === "failed";
		const isCompleted = job.status === "completed";
		const value = isFailed ? "Failed" : isCompleted ? "Complete" : progress;
		const detail = isFailed
			? String(job.error || "unknown error").trim()
			: isCompleted
				? progress
				: undefined;
		return buildHealthCard({
			key: String(job.kind || job.title || "background-maintenance"),
			label: String(job.title || job.kind || "Background maintenance"),
			value,
			detail,
			icon: isFailed ? "alert-triangle" : isCompleted ? "check-circle" : "loader",
			className: isFailed ? "status-attention" : undefined,
			title: isFailed
				? `Error: ${job.error || "unknown"}`
				: isCompleted
					? `${String(job.title || "Maintenance")} finished`
					: `${String(job.title || "Maintenance")} in progress`,
		});
	});

	const cards = [
		buildHealthCard({
			key: "overall-health",
			label: "Overall health",
			value: statusLabel,
			detail: `Weighted score ${riskScore}`,
			icon: "heart-pulse",
			className: `health-primary ${statusClass}`,
			title: drivers.length
				? `Main signals: ${drivers.join(", ")}`
				: "No major risk signals detected",
		}),
		...maintenanceCards,
		buildHealthCard({
			key: "pipeline-health",
			label: "Pipeline health",
			value: `${rawPending.toLocaleString()} pending`,
			detail: pipelineDetail,
			icon: "workflow",
			title: "Raw-event queue pressure and flush reliability",
		}),
		buildHealthCard({
			key: "retrieval-impact",
			label: "Retrieval impact",
			value: reductionLabel,
			detail: retrievalDetail,
			icon: "sparkles",
			title: "Reduction from memory reuse across recent usage",
		}),
		buildHealthCard({
			key: "data-freshness",
			label: "Data freshness",
			value: formatAgeShort(packAgeSeconds),
			detail: freshnessDetail,
			icon: "clock-3",
			title: "Recency of last memory pack activity",
		}),
	];
	renderHealthCards(healthGrid, cards);

	// Recommendations
	const recommendations: HealthAction[] = [];
	if (hasBacklog) {
		recommendations.push(
			{
				label: "Pipeline needs attention. Check queue health first.",
				command: "codemem db raw-events-status",
			},
			{
				label: "Then retry failed batches for impacted sessions.",
				command: "codemem db raw-events-retry <opencode_session_id>",
			},
		);
	}
	if (tagCoverage > 0 && tagCoverage < 0.7 && recommendations.length < 2) {
		recommendations.push({
			label: "Tag coverage is low. Preview backfill impact.",
			command: "codemem db backfill-tags --dry-run",
		});
	}
	renderActionList(healthActions, recommendations);

	healthMeta.textContent = drivers.length
		? `Why this status: ${drivers.join(", ")}.`
		: "Healthy right now. Diagnostics stay available if you want details.";
}
