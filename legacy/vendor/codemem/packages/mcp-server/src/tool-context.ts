import type { McpRpcClient } from "./rpc-client.js";

export interface CodememMcpServerOptions {
	defaultProject?: string | null;
	resolveDefaultProject?: () => string | null;
	envProject?: string | null;
	dataDir?: string;
	client?: McpRpcClient;
}

export interface ToolRegistrationContext {
	client: McpRpcClient;
	defaultProject: () => string | null;
	envProject: () => string | null;
	requestScope: string;
}
