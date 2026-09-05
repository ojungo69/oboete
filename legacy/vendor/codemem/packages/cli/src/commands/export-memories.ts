import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as p from "@clack/prompts";
import { resolveProject } from "@codemem/core";
import { Command } from "commander";
import { invokedAsTopLevelAlias } from "../command-tree.js";
import { helpStyle } from "../help-style.js";
import { addDbOption, type DbOpts, emitDeprecationWarning } from "../shared-options.js";
import { resolveOperationFilePath, runDaemonOperation } from "./daemon-operation.js";

const cmd = new Command("export-memories")
	.configureHelp(helpStyle)
	.description("Export memories to a JSON file for sharing or backup")
	.argument("<output>", "output file path (use '-' for stdout)")
	.option("--project <project>", "filter by project (defaults to git repo root)")
	.option("--all-projects", "export all projects")
	.option("--include-inactive", "include deactivated memories")
	.option("--since <iso>", "only export memories created after this ISO timestamp");

addDbOption(cmd);

cmd.action(
	async (
		output: string,
		opts: DbOpts & {
			project?: string;
			allProjects?: boolean;
			includeInactive?: boolean;
			since?: string;
		},
	) => {
		// Keep visible for this first warned release; hide the alias in a later release.
		// Suppressed for stdout export: `-` streams machine-readable JSON, so
		// stderr stays clean for piped automation.
		if (output !== "-" && invokedAsTopLevelAlias("export-memories")) {
			emitDeprecationWarning("codemem export-memories", "codemem memory export");
		}
		let temporaryDirectory: string | null = null;
		let safeToRemoveTemporary = true;
		try {
			let outputPath: string;
			if (output === "-") {
				temporaryDirectory = mkdtempSync(join(tmpdir(), "codemem-export-"));
				outputPath = join(temporaryDirectory, "export.json");
			} else {
				outputPath = resolveOperationFilePath(output);
			}
			const filters: Record<string, unknown> = {};
			if (opts.allProjects) filters.allProjects = true;
			else {
				filters.project =
					process.env.CODEMEM_PROJECT?.trim() ||
					resolveProject(process.cwd(), opts.project ?? null);
			}
			if (opts.includeInactive) filters.includeInactive = true;
			if (opts.since) filters.since = opts.since;
			safeToRemoveTemporary = false;
			const outcome = await runDaemonOperation(opts, "POST /v1/operations/export", {
				outputPath,
				filters,
			});
			safeToRemoveTemporary = outcome.ok || outcome.terminal;
			if (!outcome.ok) {
				p.log.error(`${outcome.error.message} Operation ID: ${outcome.operationId}`);
				process.exitCode = 1;
				return;
			}
			if (output === "-") {
				process.stdout.write(readFileSync(outputPath, "utf8"));
				return;
			}
			p.intro("codemem export-memories");
			p.log.success(
				[
					`Output:    ${outputPath}`,
					`Sessions:  ${Number(outcome.result.sessions ?? 0).toLocaleString()}`,
					`Memories:  ${Number(outcome.result.memory_items ?? 0).toLocaleString()}`,
					`Summaries: ${Number(outcome.result.session_summaries ?? 0).toLocaleString()}`,
					`Prompts:   ${Number(outcome.result.user_prompts ?? 0).toLocaleString()}`,
				].join("\n"),
			);
			p.outro("done");
		} catch (error) {
			p.log.error(error instanceof Error ? error.message : "Export failed");
			process.exitCode = 1;
		} finally {
			if (temporaryDirectory && safeToRemoveTemporary) {
				rmSync(temporaryDirectory, { recursive: true, force: true });
			}
		}
	},
);

export const exportMemoriesCommand = cmd;
