/* Feed summary-line text — shows counts, filters, and "scroll for more". */

import { state } from "../../../lib/state";

export function feedMetaText(visibleCount: number, hasMorePages: boolean): string {
	const filterLabel =
		state.feedTypeFilter === "observations"
			? " · observations"
			: state.feedTypeFilter === "summaries"
				? " · session summaries"
				: "";
	const filteredLabel =
		!state.feedQuery.trim() && state.lastFeedFilteredCount
			? ` · ${state.lastFeedFilteredCount} observations filtered`
			: "";
	const queryLabel = state.feedQuery.trim() ? ` · matching "${state.feedQuery.trim()}"` : "";
	const moreLabel = hasMorePages ? " · scroll for more" : "";
	return `${visibleCount} items${filterLabel}${queryLabel}${filteredLabel}${moreLabel}`;
}
