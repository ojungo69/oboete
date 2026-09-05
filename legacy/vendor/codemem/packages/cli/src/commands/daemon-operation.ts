import { randomUUID } from "node:crypto";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { hashMutationPayload } from "@codemem/core";
import { createMcpRpcClient } from "@codemem/mcp";
import type { DbOpts } from "../shared-options.js";
import { resolveDataDirOpt } from "../shared-options.js";

type OperationMethod = "POST /v1/operations/export" | "POST /v1/operations/import";
type BackupOperationMethod = "POST /v1/backup/create" | "POST /v1/backup/restore";

export type DaemonOperationRunOutcome =
	| { ok: true; operationId: string; result: Record<string, unknown> }
	| {
			ok: false;
			operationId: string;
			terminal: boolean;
			error: { code: string; message: string; retryable: boolean };
	  };

export function resolveOperationFilePath(value: string): string {
	return resolve(value.startsWith("~/") ? join(homedir(), value.slice(2)) : value);
}

export async function runDaemonOperation(
	opts: DbOpts,
	method: OperationMethod | BackupOperationMethod,
	request: Record<string, unknown>,
): Promise<DaemonOperationRunOutcome> {
	const client = createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) });
	const operationId = randomUUID();
	const submitted = await client.request(method, {
		operationId,
		payloadHash: hashMutationPayload(request),
		...request,
	});
	if (
		submitted.ok &&
		(method === "POST /v1/backup/create" || method === "POST /v1/backup/restore")
	) {
		return { ok: true, operationId, result: submitted.result };
	}

	const deadline = Date.now() + 30 * 60 * 1000;
	while (Date.now() < deadline) {
		const response = await client.request("GET /v1/operations/:id", { id: operationId });
		if (!response.ok) {
			return {
				ok: false,
				operationId,
				terminal: false,
				error: submitted.ok ? response.error : submitted.error,
			};
		}
		if (response.result.state === "committed") {
			const result = response.result.result;
			if (!result || typeof result !== "object" || Array.isArray(result)) {
				return {
					ok: false,
					operationId,
					terminal: true,
					error: {
						code: "invalid_response",
						message: "Daemon operation returned no result.",
						retryable: false,
					},
				};
			}
			return { ok: true, operationId, result: result as Record<string, unknown> };
		}
		if (response.result.state === "failed") {
			const error = response.result.error as { code?: unknown; message?: unknown } | null;
			return {
				ok: false,
				operationId,
				terminal: true,
				error: {
					code: typeof error?.code === "string" ? error.code : "operation_failed",
					message: typeof error?.message === "string" ? error.message : "Daemon operation failed.",
					retryable: false,
				},
			};
		}
		await new Promise((resolve) => setTimeout(resolve, 100));
	}
	return {
		ok: false,
		operationId,
		terminal: false,
		error: {
			code: "operation_timeout",
			message: "Daemon operation is still running.",
			retryable: false,
		},
	};
}
