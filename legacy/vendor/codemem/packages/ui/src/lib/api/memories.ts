/* Memory + summary + pack-trace endpoints — paginated list fetches
 * keyed off the current project, memory actions, and the pack-trace
 * debug call used by the Inspector. */

import {
	buildProjectParams,
	fetchJson,
	payloadError,
	readJsonPayload,
	viewerFetch,
} from "./internal";
import type { PackTrace, PaginatedResponse } from "./types";

export async function loadMemories(project: string): Promise<PaginatedResponse> {
	return loadMemoriesPage(project);
}

export async function loadMemoriesPage(
	project: string,
	options?: { limit?: number; offset?: number },
): Promise<PaginatedResponse> {
	const query = buildProjectParams(project, options?.limit, options?.offset);
	return fetchJson<PaginatedResponse>(`/api/observations?${query}`);
}

export async function loadSummaries(project: string): Promise<PaginatedResponse> {
	return loadSummariesPage(project);
}

export async function loadSummariesPage(
	project: string,
	options?: { limit?: number; offset?: number },
): Promise<PaginatedResponse> {
	const query = buildProjectParams(project, options?.limit, options?.offset);
	return fetchJson<PaginatedResponse>(`/api/summaries?${query}`);
}

export async function tracePack(payload: {
	context: string;
	project?: string | null;
	working_set_files?: string[];
	token_budget?: number | null;
	limit?: number;
}): Promise<PackTrace> {
	const resp = await viewerFetch("/api/pack/trace", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(payload),
	});
	const { text, payload: data } = await readJsonPayload<PackTrace>(resp);
	if (!resp.ok) throw new Error(payloadError(data) || text || "request failed");
	return data;
}
