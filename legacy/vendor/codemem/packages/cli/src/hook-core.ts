declare const __CODEMEM_VERSION__: string;

export const VERSION = __CODEMEM_VERSION__;

export { extractApplyPatchPaths, MUTATING_TOOL_NAMES } from "../../core/src/apply-patch.js";
export { resolveHookProject } from "../../core/src/claude-hooks.js";
export type { FileContextRetrievalAttempt } from "../../core/src/daemon-rpc-contract.js";
export {
	callDaemonRpc,
	HOOK_DELIVERY_BUDGETS,
	LOCAL_API_VERSION,
	RPC_CAPABILITY_HASH,
	RPC_MAX_BYTES,
} from "../../core/src/daemon-rpc-contract.js";
export {
	buildNormalizedEventFromClaudeHook,
	buildNormalizedEventFromCodexHook,
	NORMALIZED_EVENT_FIELDS,
	NORMALIZED_SCHEMA_VERSION,
	sealDegradedNormalizedEvent,
	validateNormalizedEvent,
} from "../../core/src/normalized-event.js";
export type { AgentMemoryConfig } from "../../core/src/redaction-pipeline.js";
export {
	parseAgentMemoryToml,
	preprocessAdapterEvent,
} from "../../core/src/redaction-pipeline.js";
export {
	REDACTION_WORKER_DEADLINE_MS,
	warmRedactionWorker,
} from "../../core/src/redaction-worker.js";
export type { RefQueryResult } from "../../core/src/ref-queries.js";
export type { SpoolRedactionMetadata } from "../../core/src/spool.js";
export { spoolMutation } from "../../core/src/spool.js";
export {
	DEFAULT_DATA_DIR,
	resolveRuntimeDataDir,
	resolveStorageLayout,
} from "../../core/src/storage-layout.js";
// 依存ゼロの文字列ユーティリティ。hook 側でも `/x+$/` の二次挙動を避けるために要る
export { isOneOf, trimEndWhere } from "../../core/src/text-trim.js";
