/**
 * Shared HTTP API response types for the codemem viewer endpoints.
 *
 * These types define the JSON contract between the Python viewer backend
 * (codemem/viewer_routes/) and the frontend (viewer_ui/src/lib/api.ts).
 *
 * ⚠️ These types are manually transcribed from Python viewer route handlers.
 * There is no automated schema validation between Python and TypeScript.
 * When modifying Python routes, update these types and add integration tests.
 *
 * Import existing store/entity types where shapes match.
 */

import type { MemoryItemResponse, PackResponse, Session, StoreStats } from "./types.js";

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/** Standard pagination envelope returned by list endpoints. */
export interface ApiPagination {
	limit: number;
	offset: number;
	next_offset: number | null;
	has_more: boolean;
}

/** Common error shape returned by all endpoints on failure. */
export interface ApiErrorResponse {
	error: string;
	detail?: string;
}

// ---------------------------------------------------------------------------
// Core viewer API responses — stats.py
// ---------------------------------------------------------------------------

/**
 * GET /api/stats
 *
 * Delegates directly to store.stats(); shape matches StoreStats.
 */
export type ApiStatsResponse = StoreStats;

/** Single usage event summary row. */
export interface ApiUsageEventSummary {
	event: string;
	total_tokens_read: number;
	total_tokens_written: number;
	total_tokens_saved: number;
	count: number;
}

/** Usage totals row. */
export interface ApiUsageTotals {
	tokens_read: number;
	tokens_written: number;
	tokens_saved: number;
	count: number;
}

/** Recent pack event row. */
export interface ApiRecentPackEvent {
	id: number;
	session_id: number | null;
	event: string;
	tokens_read: number;
	tokens_written: number;
	tokens_saved: number;
	created_at: string;
	metadata_json: Record<string, unknown> | null;
}

/**
 * GET /api/usage
 *
 * Returns usage breakdown, optionally filtered by project.
 */
export interface ApiUsageResponse {
	project: string | null;
	events: ApiUsageEventSummary[];
	totals: ApiUsageTotals;
	events_global: ApiUsageEventSummary[];
	totals_global: ApiUsageTotals;
	events_filtered: ApiUsageEventSummary[] | null;
	totals_filtered: ApiUsageTotals | null;
	recent_packs: ApiRecentPackEvent[];
}

// ---------------------------------------------------------------------------
// Core viewer API responses — memory.py
// ---------------------------------------------------------------------------

/** Extended memory item with session + ownership fields attached by the viewer. */
export interface ApiMemoryItem extends MemoryItemResponse {
	project: string | null;
	cwd?: string;
	owned_by_self?: boolean;
}

/**
 * GET /api/sessions
 *
 * Returns recent sessions with parsed metadata.
 */
export interface ApiSessionsResponse {
	items: (Session & { metadata_json: Record<string, unknown> | null })[];
}

/**
 * GET /api/projects
 *
 * Returns deduplicated, sorted project names.
 */
export interface ApiProjectsResponse {
	projects: string[];
}

/**
 * GET /api/observations  (also GET /api/memories — aliased)
 * GET /api/summaries
 *
 * Paginated memory items with session/ownership fields attached.
 */
export interface ApiMemoryListResponse {
	items: ApiMemoryItem[];
	pagination: ApiPagination;
}

/**
 * GET /api/session
 *
 * Aggregate counts for a project (or global).
 */
export interface ApiSessionCountsResponse {
	total: number;
	memories: number;
	artifacts: number;
	prompts: number;
	observations: number;
}

/**
 * GET /api/pack
 *
 * Delegates directly to store.build_memory_pack(); shape matches PackResponse.
 */
export type ApiPackResponse = PackResponse;

/**
 * GET /api/memory
 *
 * Returns a list of recent memories, optionally filtered by kind/project/scope.
 */
export interface ApiMemoryResponse {
	items: ApiMemoryItem[];
}

/**
 * GET /api/artifacts
 *
 * Returns artifacts for a given session.
 */
export interface ApiArtifactsResponse {
	items: Record<string, unknown>[];
}

/**
 * POST /api/memories/visibility — request body.
 */
export interface ApiUpdateVisibilityRequest {
	memory_id: number;
	visibility: "private" | "shared";
}

/**
 * POST /api/memories/visibility — response.
 */
export interface ApiUpdateVisibilityResponse {
	item: ApiMemoryItem;
}

// ---------------------------------------------------------------------------
// Observer status — observer_status.py
// ---------------------------------------------------------------------------

/** Active observer runtime status (from observer.get_status()). */
export interface ApiObserverActiveStatus {
	provider: string | null;
	model: string | null;
	runtime: string | null;
	auth: { method: string; token_present: boolean } | null;
	last_error?: string | null;
}

/** Per-provider credential probe result. */
export interface ApiProviderCredential {
	api_key: boolean;
	env_var: boolean;
}

/** Credential availability — provider-keyed map (from probe_available_credentials()). */
export type ApiAvailableCredentials = Record<string, ApiProviderCredential>;

/** Latest flush failure with impact annotation. */
export interface ApiFlushFailure {
	id: number;
	source: string;
	stream_id: string;
	opencode_session_id: string;
	status: string;
	error_message: string | null;
	error_type: string | null;
	observer_provider: string | null;
	observer_model: string | null;
	observer_runtime: string | null;
	observer_auth_source: string | null;
	observer_auth_type: string | null;
	observer_error_code: string | null;
	observer_error_message: string | null;
	impact: string | null;
}

/** Queue status within observer-status. */
export interface ApiObserverQueue {
	pending: number;
	sessions: number;
	auth_backoff_active: boolean;
	auth_backoff_remaining_s: number;
}

/**
 * GET /api/observer-status
 */
export interface ApiObserverStatusResponse {
	active: ApiObserverActiveStatus | null;
	capability: Record<string, unknown>;
	available_credentials: ApiAvailableCredentials;
	latest_failure: ApiFlushFailure | null;
	queue: ApiObserverQueue;
}

// ---------------------------------------------------------------------------
// Config — config.py
// ---------------------------------------------------------------------------

/**
 * GET /api/config
 */
export interface ApiConfigGetResponse {
	path: string;
	config: Record<string, unknown>;
	defaults: Record<string, unknown>;
	effective: Record<string, unknown>;
	env_overrides: Record<string, string>;
	providers: string[];
	capability: Record<string, unknown>;
}

/** Effects block in config save response. */
export interface ApiConfigEffects {
	saved_keys: string[];
	effective_keys: string[];
	hot_reloaded_keys: string[];
	restart_required_keys: string[];
	ignored_by_env_keys: string[];
	warnings: string[];
}

/**
 * POST /api/config — request body.
 * Accepts a direct updates object, or wrapped as { config: {...} }.
 * Python unwraps: `updates = payload.get("config") if "config" in payload else payload`
 */
export type ApiConfigSaveRequest = Record<string, unknown>;

/**
 * POST /api/config — response.
 */
export interface ApiConfigSaveResponse {
	path: string;
	config: Record<string, unknown>;
	effective: Record<string, unknown>;
	effects: ApiConfigEffects;
}

// ---------------------------------------------------------------------------
// Raw events — raw_events.py
// ---------------------------------------------------------------------------

/**
 * Raw event session backlog item (from store.raw_event_backlog() + _with_session_aliases).
 * Fields vary by query — max_seq/pending come from the backlog query,
 * last_received_event_seq/updated_at may be absent.
 */
export interface ApiRawEventBacklogItem {
	stream_id: string;
	opencode_session_id: string;
	session_stream_id?: string;
	session_id?: string;
	cwd?: string | null;
	project?: string | null;
	started_at?: string | null;
	max_seq?: number;
	pending?: number;
	last_seen_ts_wall_ms?: number | null;
	last_received_event_seq?: number;
	last_flushed_event_seq?: number;
	updated_at?: string;
}

/** Raw event backlog totals. */
export interface ApiRawEventBacklogTotals {
	pending: number;
	sessions: number;
}

/** Ingest capability metadata. */
export interface ApiRawEventIngestInfo {
	available: boolean;
	mode: string;
	max_body_bytes: number;
}

/**
 * GET /api/raw-events
 *
 * Returns backlog totals directly (compat endpoint for stats panel).
 */
export type ApiRawEventsResponse = ApiRawEventBacklogTotals;

/**
 * GET /api/raw-events/status
 */
export interface ApiRawEventsStatusResponse {
	items: ApiRawEventBacklogItem[];
	totals: ApiRawEventBacklogTotals;
	ingest: ApiRawEventIngestInfo;
}

/**
 * POST /api/raw-events — response.
 */
export interface ApiRawEventsPostResponse {
	inserted: number;
	received: number;
}

/**
 * POST /api/claude-hooks — response.
 */
export interface ApiClaudeHooksPostResponse {
	inserted: number;
	skipped: number;
}
