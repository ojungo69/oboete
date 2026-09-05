import { type ConfigResolutionResult, resolveCodememConfigPath } from "@codemem/core";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import {
	addConfigOption,
	addJsonOption,
	type ConfigOpts,
	type JsonOpts,
} from "../shared-options.js";

export const configCommand = new Command("config")
	.configureHelp(helpStyle)
	.description("Manage codemem configuration");

// ---------------------------------------------------------------------------
// config where — show resolved config path with full traceability
// ---------------------------------------------------------------------------

type WhereOptions = ConfigOpts & JsonOpts;

export function formatWhereHuman(result: ConfigResolutionResult): string {
	const lines: string[] = [];
	const allEntries = [result.resolved, ...result.fallbackChain];
	// Sort by original precedence order for display
	const sourceOrder: Record<string, number> = {
		"cli-flag": 0,
		"env-codemem-config": 1,
		"env-runtime-root": 2,
		"env-workspace-id": 3,
		"legacy-global": 4,
	};
	allEntries.sort((a, b) => (sourceOrder[a.source] ?? 99) - (sourceOrder[b.source] ?? 99));

	for (const entry of allEntries) {
		const isSelected = entry === result.resolved;
		const marker = isSelected ? ">>>" : "   ";
		const existsLabel = entry.exists ? "exists" : "missing";
		lines.push(
			`${marker} [${entry.source}] ${entry.path}`,
			`       ${entry.reason} (${existsLabel})`,
		);
	}
	return lines.join("\n");
}

const whereCmd = new Command("where")
	.configureHelp(helpStyle)
	.description("show resolved config file path");

addConfigOption(whereCmd);
addJsonOption(whereCmd);

whereCmd.action((opts: WhereOptions) => {
	try {
		const result = resolveCodememConfigPath(opts.config, "read");
		if (opts.json) {
			console.log(JSON.stringify(result, null, 2));
		} else {
			console.log(formatWhereHuman(result));
		}
	} catch (err) {
		if (opts.json) {
			console.log(
				JSON.stringify({
					error: "config_error",
					message: err instanceof Error ? err.message : String(err),
				}),
			);
		} else {
			console.error(err instanceof Error ? err.message : String(err));
		}
		process.exitCode = 1;
	}
});

configCommand.addCommand(whereCmd);
