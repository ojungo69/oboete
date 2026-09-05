import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as p from "@clack/prompts";
import { Command } from "commander";
import { invokedAsTopLevelAlias } from "../command-tree.js";
import { helpStyle } from "../help-style.js";
import {
	addDbOption,
	addJsonOption,
	type DbOpts,
	emitDeprecationWarning,
	emitJsonError,
	type JsonOpts,
} from "../shared-options.js";
import { resolveOperationFilePath, runDaemonOperation } from "./daemon-operation.js";

const cmd = new Command("import-memories")
	.configureHelp(helpStyle)
	.description("Import memories from an exported JSON file")
	.argument("<inputFile>", "input JSON file (use '-' for stdin)")
	.option("--remap-project <path>", "remap all projects to this path on import")
	.option("--dry-run", "preview import without writing");

addDbOption(cmd);
addJsonOption(cmd);

cmd.action(
	async (
		inputFile: string,
		opts: DbOpts &
			JsonOpts & {
				remapProject?: string;
				dryRun?: boolean;
			},
	) => {
		// Keep visible for this first warned release; hide the alias in a later release.
		// Suppressed in --json mode: the automation contract keeps stderr clean
		// for successful JSON invocations (docs/cli-design-conventions.md).
		if (!opts.json && invokedAsTopLevelAlias("import-memories")) {
			emitDeprecationWarning("codemem import-memories", "codemem memory import");
		}
		let temporaryDirectory: string | null = null;
		let safeToRemoveTemporary = true;
		try {
			let inputPath: string;
			if (inputFile === "-") {
				temporaryDirectory = mkdtempSync(join(tmpdir(), "codemem-import-"));
				inputPath = join(temporaryDirectory, "import.json");
			} else {
				inputPath = resolveOperationFilePath(inputFile);
			}
			if (inputFile === "-") {
				writeFileSync(inputPath, readFileSync(0, "utf8"), {
					encoding: "utf8",
					mode: 0o600,
					flush: true,
				});
			}
			const request: Record<string, unknown> = { inputPath };
			if (opts.remapProject) request.remapProject = opts.remapProject;
			if (opts.dryRun) request.dryRun = true;
			if (!opts.json) p.intro("codemem import-memories");
			safeToRemoveTemporary = false;
			const outcome = await runDaemonOperation(opts, "POST /v1/operations/import", request);
			safeToRemoveTemporary = outcome.ok || outcome.terminal;
			if (!outcome.ok) {
				const message = `${outcome.error.message} Operation ID: ${outcome.operationId}`;
				if (opts.json) emitJsonError(outcome.error.code, message);
				else {
					p.log.error(message);
					process.exitCode = 1;
				}
				return;
			}

			const result = outcome.result;
			if (opts.json) {
				console.log(
					JSON.stringify({
						sessions: Number(result.sessions ?? 0),
						memory_items: Number(result.memory_items ?? 0),
						skipped: result.dryRun === true,
					}),
				);
				return;
			}

			if (result.dryRun === true) {
				p.outro("dry run complete");
				return;
			}
			p.log.success(
				[
					`Imported sessions:  ${Number(result.sessions ?? 0).toLocaleString()}`,
					`Imported prompts:   ${Number(result.user_prompts ?? 0).toLocaleString()}`,
					`Imported memories:  ${Number(result.memory_items ?? 0).toLocaleString()}`,
					`Imported summaries: ${Number(result.session_summaries ?? 0).toLocaleString()}`,
				].join("\n"),
			);
			p.outro("done");
		} catch (error) {
			const message = error instanceof Error ? error.message : "Import failed";
			if (opts.json) emitJsonError("import_failed", message);
			else {
				p.log.error(message);
				process.exitCode = 1;
			}
		} finally {
			if (temporaryDirectory && safeToRemoveTemporary) {
				rmSync(temporaryDirectory, { recursive: true, force: true });
			}
		}
	},
);

export const importMemoriesCommand = cmd;
