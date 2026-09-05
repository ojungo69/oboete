import { createHash } from "node:crypto";
import { createConnection } from "node:net";

export const LOCAL_API_VERSION = 1;
export const RPC_MAX_BYTES = 32 * 1024;
export const RPC_DEFAULT_DEADLINE_MS = 2_000;
const RPC_BACKUP_DEADLINE_MS = 30 * 60 * 1_000;
export const HOOK_DELIVERY_BUDGETS = {
	claude: {
		clientHardCapMs: 2_000,
		rpcCutoffMs: 1_500,
		spoolReserveMs: 500,
		spoolLockWaitMs: 100,
		fsyncMarginMs: 400,
		outerWatchdogMs: 3_000,
	},
	codex: {
		clientHardCapMs: 1_500,
		rpcCutoffMs: 1_000,
		spoolReserveMs: 500,
		spoolLockWaitMs: 100,
		fsyncMarginMs: 400,
		outerWatchdogMs: 5_000,
	},
} as const;

export const RPC_METHODS = [
	"GET /v1/health",
	"GET /v1/doctor",
	"POST /v1/events",
	"POST /v1/events/batch",
	"POST /v1/context/pack",
	"POST /v1/search",
	"POST /v1/retrieval/file-context",
	"POST /v1/retrieval/file-context/delivery",
	"GET /v1/memories/:id",
	"POST /v1/memories/record",
	"DELETE /v1/memories/:id",
	"GET /v1/checkpoints",
	"GET /v1/view",
	"POST /v1/viewer/auth/nonce",
	"POST /v1/viewer/auth/exchange",
	"POST /v1/viewer/auth/verify",
	"POST /v1/viewer/auth/logout",
	"GET /v1/backup/list",
	"POST /v1/backup/create",
	"POST /v1/backup/verify",
	"POST /v1/backup/restore",
	"POST /v1/operations/export",
	"POST /v1/operations/import",
	"GET /v1/operations/:id",
	"POST /v1/jobs",
	"GET /v1/jobs",
	"GET /v1/jobs/:id",
	"GET /v1/processing-jobs/:id",
	"POST /v1/processing-jobs/:id/doctor-retry",
] as const;

export type RpcMethod = (typeof RPC_METHODS)[number];

const BACKUP_RPC_METHODS = new Set<string>([
	"GET /v1/backup/list",
	"POST /v1/backup/create",
	"POST /v1/backup/verify",
	"POST /v1/backup/restore",
]);

export function rpcDeadlineForMethod(method: string): number {
	return BACKUP_RPC_METHODS.has(method) ? RPC_BACKUP_DEADLINE_MS : RPC_DEFAULT_DEADLINE_MS;
}

export type FileContextRetrievalAttempt = {
	attemptId: string;
	startedAt: string;
	completedAt: string;
	retrievalStatus: "succeeded" | "no_results" | "skipped" | "failed";
	candidateIds?: number[];
	candidateCount?: number;
	selectedIds?: number[];
	failureCode?: string;
	failureStage?: string;
	project?: string | null;
	repositoryPath?: string;
	sourceSessionId?: string | null;
};

export const RPC_CAPABILITY_HASH = createHash("sha256")
	.update(RPC_METHODS.join("\n"))
	.digest("hex");

export type TypedRpcError = {
	error: { code: string; message: string; retryable: boolean };
};

export function typedError(code: string, message: string, retryable = false): TypedRpcError {
	return { error: { code, message, retryable } };
}

export type RpcRequest = {
	id: string;
	method: string;
	adapter_version: string;
	native_cli_version: string;
	normalized_schema_version: number;
	local_api_version: number;
	capability_hash: string;
	body?: Record<string, unknown>;
};

export type RpcSuccess = {
	id: string;
	result: Record<string, unknown>;
};

export function mapPeerConnectError(error: NodeJS.ErrnoException): TypedRpcError {
	if (error.code === "EACCES") {
		return typedError("peer_denied", "Peer is not allowed to connect to the daemon socket.");
	}
	if (error.code === "ECONNREFUSED" || error.code === "ENOENT") {
		return typedError("daemon_unavailable", "Daemon is not running.", true);
	}
	return typedError("peer_denied", error.message || "Peer connection failed.");
}

export function callDaemonRpc(
	socketPath: string,
	request: RpcRequest,
	options?: { timeoutMs?: number; signal?: AbortSignal; maxResponseBytes?: number },
): Promise<RpcSuccess | TypedRpcError> {
	return new Promise((resolve, reject) => {
		const socket = createConnection(socketPath);
		const signal = options?.signal;
		let abortListener: (() => void) | undefined;
		let settled = false;
		const finish = (error?: Error, value?: RpcSuccess | TypedRpcError) => {
			if (settled) return;
			settled = true;
			if (abortListener) signal?.removeEventListener("abort", abortListener);
			socket.destroy();
			if (error) reject(error);
			else resolve(value as RpcSuccess | TypedRpcError);
		};
		abortListener = () => finish(new Error("RPC client aborted"));
		if (signal?.aborted) abortListener();
		else signal?.addEventListener("abort", abortListener, { once: true });
		const chunks: Buffer[] = [];
		let responseBytes = 0;
		socket.setTimeout(options?.timeoutMs ?? RPC_DEFAULT_DEADLINE_MS);
		socket.once("connect", () => {
			socket.write(`${JSON.stringify(request)}\n`);
		});
		socket.on("data", (chunk: Buffer) => {
			const newline = chunk.indexOf(0x0a);
			const responseChunk = newline < 0 ? chunk : chunk.subarray(0, newline + 1);
			responseBytes += responseChunk.length;
			if (options?.maxResponseBytes !== undefined && responseBytes > options.maxResponseBytes) {
				finish(new Error(`RPC response exceeds ${options.maxResponseBytes} bytes`));
				return;
			}
			chunks.push(responseChunk);
			if (newline < 0) return;
			const buffer = Buffer.concat(chunks, responseBytes);
			try {
				finish(
					undefined,
					JSON.parse(buffer.subarray(0, -1).toString("utf8")) as RpcSuccess | TypedRpcError,
				);
			} catch (error) {
				finish(error instanceof Error ? error : new Error(String(error)));
			}
		});
		socket.once("error", (error) => {
			const peerError = error as NodeJS.ErrnoException;
			if (
				peerError.code === "EACCES" ||
				peerError.code === "ECONNREFUSED" ||
				peerError.code === "ENOENT"
			) {
				finish(undefined, mapPeerConnectError(peerError));
				return;
			}
			finish(peerError);
		});
		socket.once("timeout", () => finish(new Error("RPC client timed out")));
		socket.once("close", () => {
			if (!settled) finish(new Error("RPC connection closed without a response"));
		});
	});
}
