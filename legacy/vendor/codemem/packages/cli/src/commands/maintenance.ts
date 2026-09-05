import * as p from "@clack/prompts";
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

type Job = {
	jobId: string;
	kind: string;
	state: string;
	attempts: number;
	maxAttempts: number;
	submittedAt: string;
	startedAt: string | null;
	finishedAt: string | null;
	error: { message?: string } | null;
};

const maintenanceCmd = new Command("maintenance")
	.configureHelp(helpStyle)
	.description("Inspect daemon maintenance / backfill jobs");

const statusCmd = new Command("status")
	.configureHelp(helpStyle)
	.description("Print current daemon job status");

addDbOption(statusCmd);
addJsonOption(statusCmd);

statusCmd.action(async (opts: DbOpts & JsonOpts) => {
	const outcome = await createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) }).request(
		"GET /v1/jobs",
		{},
	);
	if (!outcome.ok) {
		if (opts.json) emitJsonError(outcome.error.code, outcome.error.message);
		else {
			p.log.error(outcome.error.message);
			process.exitCode = 1;
		}
		return;
	}
	const jobs = outcome.result.jobs as Job[];
	if (opts.json) {
		console.log(JSON.stringify({ jobs }, null, 2));
		return;
	}
	printJobs(jobs);
});

maintenanceCmd.addCommand(statusCmd);

export const maintenanceCommand = maintenanceCmd;

function printJobs(jobs: Job[]): void {
	p.intro("codemem maintenance");
	if (jobs.length === 0) {
		p.log.info("No daemon jobs recorded.");
		p.outro("done");
		return;
	}
	for (const job of jobs) {
		const when = job.finishedAt ?? job.startedAt ?? job.submittedAt;
		const message = `${job.kind} · ${job.state} · attempt ${job.attempts}/${job.maxAttempts} · ${when}`;
		if (job.state === "failed") p.log.error(`${message}\n${job.error?.message ?? "Job failed"}`);
		else if (job.state === "completed") p.log.success(message);
		else p.log.step(message);
	}
	p.outro("done");
}
