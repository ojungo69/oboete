import { randomUUID } from "node:crypto";
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

const cmd = new Command("recent")
	.configureHelp(helpStyle)
	.description("Show recent memories")
	.option("--limit <n>", "max results", "5")
	.option("--project <project>", "project identifier (defaults to git repo root)")
	.option("--all-projects", "search across all projects")
	.option("--kind <kind>", "filter by memory kind");

addDbOption(cmd);
addJsonOption(cmd);

cmd.action(
	async (
		opts: DbOpts &
			JsonOpts & {
				limit: string;
				project?: string;
				allProjects?: boolean;
				kind?: string;
			},
	) => {
		try {
			const limit = Math.max(1, Number.parseInt(opts.limit, 10) || 5);
			const filters: { kind?: string; project?: string } = {};
			if (opts.kind) filters.kind = opts.kind;
			if (!opts.allProjects) {
				const defaultProject = process.env.CODEMEM_PROJECT?.trim() || null;
				const project = defaultProject || resolveProject(process.cwd(), opts.project ?? null);
				if (project) filters.project = project;
			}
			const outcome = await createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) }).request(
				"POST /v1/search",
				{ requestId: randomUUID(), mode: "recent", filters, limit },
			);
			if (!outcome.ok) {
				if (opts.json) emitJsonError(outcome.error.code, outcome.error.message);
				else {
					console.error(outcome.error.message);
					process.exitCode = 1;
				}
				return;
			}
			const items = outcome.result.items as Array<{ id: number; kind: string; title: string }>;
			if (opts.json) {
				console.log(JSON.stringify(items));
			} else {
				for (const item of items) {
					console.log(`#${item.id} [${item.kind}] ${item.title}`);
				}
			}
		} catch (error) {
			const message = error instanceof Error ? error.message : "Recent lookup failed";
			if (opts.json) {
				emitJsonError("recent_failed", message);
			} else {
				console.error(message);
				process.exitCode = 1;
			}
			return;
		}
	},
);

export const recentCommand = cmd;
