import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import {
	addDbOption,
	addJsonOption,
	emitFeatureUnavailable,
	type JsonOpts,
} from "../shared-options.js";

function collectKind(value: string, previous: string[]): string[] {
	return [...previous, ...value.split(",")].map((kind) => kind.trim()).filter(Boolean);
}

const cmd = new Command("distill")
	.configureHelp(helpStyle)
	.description("Mine recurring memories into reviewable context candidates")
	.option("-p, --project <project>", "project identifier (defaults to git repo root)")
	.option("-A, --all-projects", "mine memories across all projects")
	.option("-k, --kind <kind>", "memory kind to mine (repeat or comma-separate)", collectKind, [])
	.option("-m, --min-recurrence <n>", "minimum member count per candidate", "2")
	.option("-l, --limit <n>", "max candidates", "10")
	.option("-e, --explain", "include evidence snippets in human output")
	.option("--include-documented", "include candidates already represented in context files")
	.option(
		"--no-judge",
		"skip the observer-model worthiness judgment that drops routine-activity clusters",
	)
	.option("-D, --draft", "draft an AGENTS.md rule for the top candidate and show the diff")
	.option(
		"--apply",
		"write the drafted rule to the target file (implies --draft; prompts to confirm; with --json applies immediately without prompting)",
	);

addDbOption(cmd);
addJsonOption(cmd);
cmd.action((opts: JsonOpts) => emitFeatureUnavailable("distill", 6, opts.json));

export const distillCommand = cmd;
