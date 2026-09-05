import { randomUUID } from "node:crypto";
import {
	callDaemonRpc,
	LOCAL_API_VERSION,
	NORMALIZED_SCHEMA_VERSION,
	RPC_CAPABILITY_HASH,
	type RpcMethod,
	resolveRuntimeDataDir,
	resolveStorageLayout,
	VERSION,
} from "@codemem/core";

export type ViewerRpcCall = (
	method: RpcMethod,
	body?: Record<string, unknown>,
) => Promise<Record<string, unknown>>;

export class ViewerRpcError extends Error {
	constructor(
		readonly code: string,
		message: string,
		readonly retryable = false,
	) {
		super(message);
		this.name = "ViewerRpcError";
	}
}

export function createViewerRpcCall(options?: {
	socketPath?: string;
	timeoutMs?: number;
}): ViewerRpcCall {
	const dataDir = resolveRuntimeDataDir();
	const socketPath = options?.socketPath ?? resolveStorageLayout(dataDir).socketPath;
	return async (method, body = {}) => {
		try {
			const response = await callDaemonRpc(
				socketPath,
				{
					id: randomUUID(),
					method,
					adapter_version: VERSION,
					native_cli_version: VERSION,
					normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
					local_api_version: LOCAL_API_VERSION,
					capability_hash: RPC_CAPABILITY_HASH,
					body,
				},
				{ timeoutMs: options?.timeoutMs },
			);
			if ("error" in response) {
				throw new ViewerRpcError(
					response.error.code,
					response.error.message,
					response.error.retryable,
				);
			}
			return response.result;
		} catch (error) {
			if (error instanceof ViewerRpcError) throw error;
			throw new ViewerRpcError("daemon_unavailable", "Daemon is not running.", true);
		}
	};
}
