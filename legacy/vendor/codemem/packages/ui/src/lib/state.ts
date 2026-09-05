/* Global application state — shared across tabs. */

export type RefreshState = "idle" | "refreshing" | "paused" | "error";
export type CanonicalTabId = "feed" | "health";
export type TabId = CanonicalTabId;
export type RoutableTabId = TabId;
export const ALL_TAB_IDS: CanonicalTabId[] = ["feed", "health"];

/* ── Cached server payload shapes ─────────────────────────── */

/**
 * Minimal interfaces covering the fields UI code actually reads from the
 * viewer API responses that get cached in `state`. All fields are optional
 * and shapes are open (additional fields from the server are just ignored).
 * When the UI starts reading a new field, add it here.
 */

export interface UsageTotals {
	tokens_read?: number;
	tokens_saved?: number;
	work_investment_tokens?: number;
}

export interface RecentPack {
	created_at?: string;
	tokens_read?: number;
	tokens_saved?: number;
	metadata_json?: {
		exact_duplicates_collapsed?: number;
		exact_dedupe_enabled?: boolean;
	};
}

export interface CachedStatsPayload {
	identity?: { actor_id?: string };
	database?: {
		path?: string;
		size_bytes?: number;
		active_memory_items?: number;
		vector_coverage?: number;
		tags_coverage?: number;
	};
	usage?: { totals?: UsageTotals };
	reliability?: {
		counts?: { errored_batches?: number };
		rates?: {
			flush_success_rate?: number;
			dropped_event_rate?: number;
		};
	};
	maintenance_jobs?: unknown[];
}

export interface CachedUsagePayload {
	totals_global?: UsageTotals;
	totals?: UsageTotals;
	totals_filtered?: UsageTotals | null;
	events?: unknown[];
	recent_packs?: RecentPack[];
}

export interface CachedRawEventsPayload {
	pending?: number;
	sessions?: number;
	events?: unknown;
}

const TAB_KEY = "codemem-tab";
const FEED_FILTER_KEY = "codemem-feed-filter";
const DETAILS_OPEN_KEY = "codemem-details-open";

export const FEED_FILTERS = ["all", "observations", "summaries"] as const;
export type FeedFilter = (typeof FEED_FILTERS)[number];

/* ── Mutable application state ─────────────────────────────── */

export const state = {
	/* Tab */
	activeTab: "feed" as TabId,

	/* Project filter */
	currentProject: "",

	/* Refresh */
	refreshState: "idle" as RefreshState,
	refreshInFlight: false,
	refreshQueued: false,
	refreshTimer: null as ReturnType<typeof setInterval> | null,

	/* Feed */
	feedTypeFilter: "all" as FeedFilter,
	feedQuery: "",
	lastFeedItems: [] as unknown[],
	lastFeedFilteredCount: 0,
	lastFeedSignature: "",
	pendingFeedItems: null as unknown[] | null,

	/* Feed item view state */
	itemViewState: new Map<string, string>(),
	itemExpandState: new Map<string, boolean>(),
	newItemKeys: new Set<string>(),

	/* Cached payloads */
	lastStatsPayload: null as CachedStatsPayload | null,
	lastUsagePayload: null as CachedUsagePayload | null,
	lastRawEventsPayload: null as CachedRawEventsPayload | null,

	/**
	 * Project names cached from the most recent /api/projects fetch.
	 * Populated on app boot and reused for the header project filter.
	 */
	knownProjects: [] as string[],

	/* Config */
	configDefaults: {} as Record<string, unknown>,
	configPath: "",
	settingsDirty: false,
};

export function resolveAccessibleTab(tab: RoutableTabId): CanonicalTabId {
	return ALL_TAB_IDS.includes(tab) ? tab : "feed";
}

/* ── Persistence helpers ───────────────────────────────────── */

/** Parse `window.location.hash` into its canonical top-level tab. */
export function parseTabFromHash(hash = window.location.hash): CanonicalTabId | null {
	const first = hash.replace(/^#/, "").split("/")[0] as RoutableTabId;
	return ALL_TAB_IDS.includes(first as CanonicalTabId) ? (first as CanonicalTabId) : null;
}

export function getActiveTab(): CanonicalTabId {
	const fromHash = parseTabFromHash();
	if (fromHash) return resolveAccessibleTab(fromHash);
	const saved = localStorage.getItem(TAB_KEY);
	if (saved && ALL_TAB_IDS.includes(saved as CanonicalTabId)) {
		return resolveAccessibleTab(saved as RoutableTabId);
	}
	return "feed";
}

export function setActiveTab(tab: RoutableTabId, options: { canonicalHash?: boolean } = {}) {
	const nextTab = resolveAccessibleTab(tab);
	state.activeTab = nextTab;
	localStorage.setItem(TAB_KEY, nextTab);
	// Only rewrite the hash when the top-level tab actually changed, so boot
	// (`initTabs()` re-setting the already-active tab) does not clobber
	// sub-view fragments.
	const currentTop = parseTabFromHash();
	if (options.canonicalHash) {
		window.location.hash = nextTab;
	} else if (currentTop !== nextTab) {
		window.location.hash = nextTab;
	}
}

export function getFeedTypeFilter(): FeedFilter {
	const saved = localStorage.getItem(FEED_FILTER_KEY) || "all";
	return FEED_FILTERS.includes(saved as FeedFilter) ? (saved as FeedFilter) : "all";
}

export function setFeedTypeFilter(value: string) {
	state.feedTypeFilter = FEED_FILTERS.includes(value as FeedFilter) ? (value as FeedFilter) : "all";
	localStorage.setItem(FEED_FILTER_KEY, state.feedTypeFilter);
}

export function isDetailsOpen(): boolean {
	return localStorage.getItem(DETAILS_OPEN_KEY) === "1";
}

export function setDetailsOpen(open: boolean) {
	localStorage.setItem(DETAILS_OPEN_KEY, open ? "1" : "0");
}

/* ── Init from storage ─────────────────────────────────────── */

export function initState() {
	state.activeTab = getActiveTab();
	state.feedTypeFilter = getFeedTypeFilter();
}
