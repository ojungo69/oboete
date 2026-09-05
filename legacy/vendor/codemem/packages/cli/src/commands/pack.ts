import { randomUUID } from "node:crypto";
import * as p from "@clack/prompts";
import type { PackResponse, PackTrace } from "@codemem/core";
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
import { addPackRequestOptions, buildPackRequestOptions, PackUsageError } from "./pack-shared.js";

type PackCommandOptions = DbOpts &
	JsonOpts & {
		limit: string;
		budget?: string;
		tokenBudget?: string;
		workingSetFile?: string[];
		project?: string;
		allProjects?: boolean;
		compact?: boolean;
		compactDetail?: string;
	};

function describeCandidate(candidate: PackTrace["retrieval"]["candidates"][number]): string[] {
	const scoreParts = [
		candidate.scores.combined_score != null
			? `combined=${candidate.scores.combined_score.toFixed(2)}`
			: null,
		candidate.scores.base_score != null ? `base=${candidate.scores.base_score.toFixed(2)}` : null,
		candidate.scores.text_overlap > 0 ? `text=${candidate.scores.text_overlap}` : null,
		candidate.scores.tag_overlap > 0 ? `tag=${candidate.scores.tag_overlap}` : null,
		candidate.scores.working_set_overlap > 0
			? `working_set=${candidate.scores.working_set_overlap.toFixed(2)}`
			: null,
	]
		.filter(Boolean)
		.join(" ");

	const lines = [`${candidate.rank}. [${candidate.id}] (${candidate.kind}) ${candidate.title}`];
	if (candidate.section) lines.push(`   - section: ${candidate.section}`);
	if (candidate.reasons.length > 0) lines.push(`   - reasons: ${candidate.reasons.join(", ")}`);
	if (scoreParts) lines.push(`   - scores: ${scoreParts}`);
	if (candidate.preview) lines.push(`   - preview: ${candidate.preview}`);
	return lines;
}

export function renderPackTrace(trace: PackTrace): string {
	const workingSet =
		trace.inputs.working_set_files.length > 0
			? trace.inputs.working_set_files.join(", ")
			: "(none)";
	const lines = [
		"Pack trace",
		`- Query: ${trace.inputs.query}`,
		...(trace.inputs.sanitized_query ? [`- Sanitized query: ${trace.inputs.sanitized_query}`] : []),
		`- Project: ${trace.inputs.project ?? "(default)"}`,
		`- Working set: ${workingSet}`,
		`- Mode: ${trace.mode.selected}`,
		`- Mode reasons: ${trace.mode.reasons.join(", ") || "(none)"}`,
		`- Token budget: ${trace.inputs.token_budget ?? "(none)"}`,
		"",
	];

	for (const disposition of ["selected", "dropped", "deduped", "trimmed"] as const) {
		const group = trace.retrieval.candidates.filter(
			(candidate) => candidate.disposition === disposition,
		);
		if (group.length === 0) continue;
		lines.push(disposition.charAt(0).toUpperCase() + disposition.slice(1));
		for (const candidate of group) {
			lines.push(...describeCandidate(candidate));
		}
		lines.push("");
	}

	lines.push(
		"Assembly",
		`- deduped ids: ${trace.assembly.deduped_ids.join(", ") || "(none)"}`,
		`- trimmed ids: ${trace.assembly.trimmed_ids.join(", ") || "(none)"}`,
		`- trim reasons: ${trace.assembly.trim_reasons.join(", ") || "(none)"}`,
		`- section counts: summary=${trace.output.section_counts.summary} timeline=${trace.output.section_counts.timeline} observations=${trace.output.section_counts.observations}`,
		`- estimated tokens: ${trace.output.estimated_tokens}`,
		`- truncated: ${trace.output.truncated ? "yes" : "no"}`,
		"",
		"Final pack",
		trace.output.pack_text,
	);
	return lines.join("\n");
}

async function packAction(context: string, opts: PackCommandOptions): Promise<void> {
	try {
		const { limit, budget, filters, renderOptions } = buildPackRequestOptions(opts, {
			envProject: process.env.CODEMEM_PROJECT,
		});
		if (renderOptions !== undefined && Object.keys(renderOptions).length > 0) {
			throw new PackUsageError("pack rendering overrides are unavailable during Phase 1");
		}
		const outcome = await createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) }).request(
			"POST /v1/context/pack",
			{ requestId: randomUUID(), context, limit, tokenBudget: budget, filters, trace: false },
		);
		if (!outcome.ok) {
			if (opts.json) emitJsonError(outcome.error.code, outcome.error.message);
			else {
				p.log.error(outcome.error.message);
				process.exitCode = 1;
			}
			return;
		}
		const result = outcome.result.pack as PackResponse;
		emitPackResult(context, opts, result);
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		const usageError = error instanceof PackUsageError;
		if (opts.json)
			emitJsonError(usageError ? "usage_error" : "pack_failed", message, usageError ? 2 : 1);
		else {
			p.log.error(message);
			process.exitCode = usageError ? 2 : 1;
		}
	}
}

function emitPackResult(
	context: string,
	opts: Pick<PackCommandOptions, "json">,
	result: PackResponse,
): void {
	if (opts.json) {
		console.log(JSON.stringify(result, null, 2));
		return;
	}

	p.intro(`Memory pack for "${context}"`);

	if (result.items.length === 0) {
		p.log.warn("No relevant memories found.");
		p.outro("done");
		return;
	}

	const metrics = result.metrics;
	p.log.info(
		`${metrics.total_items} items, ~${metrics.pack_tokens} tokens` +
			(metrics.fallback_used ? " (fallback)" : "") +
			`  [fts:${metrics.sources.fts} sem:${metrics.sources.semantic} fuzzy:${metrics.sources.fuzzy}]`,
	);

	for (const item of result.items) {
		p.log.step(`#${item.id}  ${item.kind}  ${item.title}`);
	}

	p.note(result.pack_text, "pack_text");
	p.outro("done");
}

async function traceAction(context: string, opts: PackCommandOptions): Promise<void> {
	try {
		const { limit, budget, filters, renderOptions } = buildPackRequestOptions(opts, {
			envProject: process.env.CODEMEM_PROJECT,
		});
		if (renderOptions !== undefined && Object.keys(renderOptions).length > 0) {
			throw new PackUsageError("pack rendering overrides are unavailable during Phase 1");
		}
		const outcome = await createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) }).request(
			"POST /v1/context/pack",
			{ requestId: randomUUID(), context, limit, tokenBudget: budget, filters, trace: true },
		);
		if (!outcome.ok) {
			if (opts.json) emitJsonError(outcome.error.code, outcome.error.message);
			else {
				p.log.error(outcome.error.message);
				process.exitCode = 1;
			}
			return;
		}
		const trace = outcome.result.trace as PackTrace;

		if (opts.json) {
			console.log(JSON.stringify(trace, null, 2));
			return;
		}

		console.log(renderPackTrace(trace));
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		const usageError = error instanceof PackUsageError;
		if (opts.json)
			emitJsonError(usageError ? "usage_error" : "pack_trace_failed", message, usageError ? 2 : 1);
		else {
			p.log.error(message);
			process.exitCode = usageError ? 2 : 1;
		}
	}
}

const packCmd = addPackRequestOptions(
	new Command("pack")
		.enablePositionalOptions()
		.configureHelp(helpStyle)
		.description("Build a context-aware memory pack")
		.argument("<context>", "context string to search for"),
);
addDbOption(packCmd);
addJsonOption(packCmd);
packCmd.action(packAction);

const traceCmd = addPackRequestOptions(
	new Command("trace")
		.configureHelp(helpStyle)
		.description("Trace retrieval and assembly for a memory pack")
		.argument("<context>", "context string to trace"),
);

addDbOption(traceCmd);
addJsonOption(traceCmd);
traceCmd.action(traceAction);
packCmd.addCommand(traceCmd);

export const packCommand = packCmd;
