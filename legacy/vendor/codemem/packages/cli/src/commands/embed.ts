import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import {
	addDbOption,
	addJsonOption,
	emitFeatureUnavailable,
	type JsonOpts,
} from "../shared-options.js";

const embedCmd = new Command("embed")
	.configureHelp(helpStyle)
	.description("Backfill semantic embeddings")
	.option("--limit <n>", "max memories to check")
	.option("--since <iso>", "only memories created at/after this ISO timestamp")
	.option("--project <project>", "project identifier (defaults to git repo root)")
	.option("--all-projects", "embed across all projects")
	.option("--inactive", "include inactive memories")
	.option("--dry-run", "preview work without writing vectors");

addDbOption(embedCmd);
addJsonOption(embedCmd);

export const embedCommand = embedCmd.action((opts: JsonOpts) =>
	emitFeatureUnavailable("embed", 7, opts.json),
);
