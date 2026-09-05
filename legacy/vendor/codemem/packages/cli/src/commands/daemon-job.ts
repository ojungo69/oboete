import { createMcpRpcClient } from "@codemem/mcp";
import type { DbOpts } from "../shared-options.js";
import { resolveDataDirOpt } from "../shared-options.js";

export type DaemonJobRunOutcome =
	| { ok: true; jobId: string; result: unknown }
	| {
			ok: false;
			jobId: string | null;
			error: { code: string; message: string; retryable: boolean };
	  };

export async function runDaemonJob(
	opts: DbOpts,
	kind: string,
	args: Record<string, unknown>,
	dryRun = false,
): Promise<DaemonJobRunOutcome> {
	const client = createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) });
	const submitted = await client.request("POST /v1/jobs", { kind, args, dryRun });
	if (!submitted.ok) return { ok: false, jobId: null, error: submitted.error };
	const jobId = submitted.result.jobId;
	if (typeof jobId !== "string") {
		return {
			ok: false,
			jobId: null,
			error: {
				code: "invalid_response",
				message: "Daemon did not return a job ID.",
				retryable: false,
			},
		};
	}

	const deadline = Date.now() + 30 * 60 * 1000;
	while (Date.now() < deadline) {
		const response = await client.request("GET /v1/jobs/:id", { id: jobId });
		if (!response.ok) return { ok: false, jobId, error: response.error };
		const job = response.result.job;
		if (!job || typeof job !== "object" || Array.isArray(job)) {
			return {
				ok: false,
				jobId,
				error: { code: "job_not_found", message: "Daemon job was not found.", retryable: false },
			};
		}
		const record = job as Record<string, unknown>;
		if (record.state === "completed") return { ok: true, jobId, result: record.result };
		if (record.state === "failed") {
			const error = record.error as { code?: unknown; message?: unknown } | null;
			return {
				ok: false,
				jobId,
				error: {
					code: typeof error?.code === "string" ? error.code : "job_failed",
					message: typeof error?.message === "string" ? error.message : "Daemon job failed.",
					retryable: false,
				},
			};
		}
		await new Promise((resolve) => setTimeout(resolve, 100));
	}
	return {
		ok: false,
		jobId,
		error: {
			code: "job_timeout",
			message: "Daemon job is still running; inspect it with `codemem maintenance status`.",
			retryable: false,
		},
	};
}
