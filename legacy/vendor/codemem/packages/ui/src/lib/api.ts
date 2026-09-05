/* API fetch wrappers — barrel re-export of the split modules. The
 * viewer HTTP endpoints were broken up into api/ (types.ts,
 * internal.ts, runtime.ts, stats.ts, memories.ts, config.ts) during
 * the decomposition; this file now only keeps the public API stable
 * for `import * as api from "../lib/api"` call sites. */

export { loadConfig, loadObserverStatus } from "./api/config";
export {
	loadMemories,
	loadMemoriesPage,
	loadSummaries,
	loadSummariesPage,
	tracePack,
} from "./api/memories";
export { loadProjects, loadRuntimeInfo, pingViewerReady } from "./api/runtime";
export { loadRawEvents, loadSession, loadStats, loadUsage } from "./api/stats";
export type {
	PackTrace,
	PackTraceCandidate,
	PaginatedResponse,
	RuntimeInfo,
} from "./api/types";
