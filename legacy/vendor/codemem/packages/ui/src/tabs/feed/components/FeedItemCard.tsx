import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { Chip } from "../../../components/primitives/chip";
import { Tooltip } from "../../../components/primitives/tooltip";
import { highlightText } from "../../../lib/dom";
import {
	formatDate,
	formatFileList,
	formatRelativeTime,
	formatTagLabel,
	parseJsonArray,
} from "../../../lib/format";
import { state } from "../../../lib/state";
import {
	renderFactsContent,
	renderNarrativeContent,
	renderSummarySections,
} from "../data/body-renderers";
import { authorLabel, itemKey, mergeMetadata, trustStateLabel } from "../data/helpers";
import {
	clampClass,
	defaultObservationView,
	observationViewData,
	observationViewModes,
	shouldClampBody,
} from "../data/observation-view";
import { canonicalKind, getSummaryObject, isSummaryLikeItem } from "../data/summary-extract";
import type { FeedItem, ItemViewMode } from "../types";
import { FeedViewToggle } from "./FeedViewToggle";
import { ProvenanceChip } from "./ProvenanceChip";
import { TagChip } from "./TagChip";

export interface FeedItemCardProps {
	item: FeedItem;
}

export function FeedItemCard({ item }: FeedItemCardProps) {
	const metadata = mergeMetadata(item?.metadata_json);
	const isSessionSummary = isSummaryLikeItem(item, metadata);
	const displayKindValue = canonicalKind(item, metadata);
	const rowKey = itemKey(item);
	const defaultTitle = item.title || "(untitled)";
	const displayTitle = isSessionSummary && metadata?.request ? metadata.request : defaultTitle;
	const createdAtRaw = item.created_at || item.created_at_utc;
	const relative = formatRelativeTime(createdAtRaw);
	const tags = parseJsonArray(item.tags || []);
	const files = parseJsonArray(item.files || []);
	const project = item.project || "";
	const actor = authorLabel(item);
	const visibility = String(item.visibility || metadata?.visibility || "private").trim();
	const workspaceKind = String(item.workspace_kind || metadata?.workspace_kind || "").trim();
	const originSource = String(item.origin_source || metadata?.origin_source || "").trim();
	const originDeviceId = String(item.origin_device_id || metadata?.origin_device_id || "").trim();
	const trustState = String(item.trust_state || metadata?.trust_state || "").trim();
	const tagContent = tags.length ? ` · ${tags.map((t) => formatTagLabel(t)).join(", ")}` : "";
	const fileContent = files.length ? ` · ${formatFileList(files)}` : "";
	const memoryId = Number(item.id || 0);
	const memoryIdLabel = memoryId > 0 ? `ID ${memoryId}` : "";
	const [isNew, setIsNew] = useState(state.newItemKeys.has(rowKey));
	const summaryObj = isSessionSummary
		? getSummaryObject({ ...item, metadata_json: metadata })
		: null;
	const observationData = !isSessionSummary
		? observationViewData({ ...item, metadata_json: metadata })
		: null;
	const modes = observationData ? observationViewModes(observationData) : [];
	const fallbackMode = observationData ? defaultObservationView(observationData) : "summary";
	const storedMode = state.itemViewState.get(rowKey) as ItemViewMode | undefined;
	const initialMode =
		observationData && storedMode && modes.some((mode) => mode.id === storedMode)
			? storedMode
			: (fallbackMode as ItemViewMode);
	const [activeMode, setActiveMode] = useState<ItemViewMode>(initialMode);
	const activeExpandKey = `${rowKey}:${activeMode}`;
	const [expanded, setExpanded] = useState(state.itemExpandState.get(activeExpandKey) === true);
	const summarySections = summaryObj ? renderSummarySections(summaryObj) : [];

	useEffect(() => {
		if (!observationData) return;
		if (modes.some((mode) => mode.id === activeMode)) return;
		setActiveMode(fallbackMode as ItemViewMode);
	}, [activeMode, fallbackMode, modes, observationData]);

	useEffect(() => {
		state.itemViewState.set(rowKey, activeMode);
	}, [activeMode, rowKey]);

	useEffect(() => {
		const nextExpandKey = `${rowKey}:${activeMode}`;
		setExpanded(state.itemExpandState.get(nextExpandKey) === true);
	}, [activeMode, rowKey]);

	useEffect(() => {
		if (!isNew) return;
		const timer = window.setTimeout(() => {
			state.newItemKeys.delete(rowKey);
			setIsNew(false);
		}, 700);
		return () => window.clearTimeout(timer);
	}, [isNew, rowKey]);

	const secondaryMeta = [project ? `Project ${project}` : "No project", relative]
		.filter(Boolean)
		.join(" · ");
	const provenanceDetails = [
		workspaceKind && workspaceKind !== visibility ? `Workspace ${workspaceKind}` : "",
		originSource ? `From ${originSource}` : "",
		originDeviceId && actor !== "You" ? `Device ${originDeviceId}` : "",
		trustState && trustState !== "trusted" ? trustStateLabel(trustState) : "",
	]
		.filter(Boolean)
		.join(" · ");
	const metaText = [`${tagContent}${fileContent}`.trim(), provenanceDetails]
		.filter(Boolean)
		.join(" · ");

	const canClamp = Boolean(observationData) && shouldClampBody(activeMode, observationData);
	const bodyClassName = [
		activeMode === "facts" ? "feed-body facts" : "feed-body",
		canClamp && !expanded ? clampClass(activeMode).join(" ") : "",
	]
		.filter(Boolean)
		.join(" ");

	const bodyContent = isSessionSummary
		? summarySections.length
			? h("div", { className: "feed-body facts" }, summarySections)
			: renderNarrativeContent(String(item.body_text || "")) || h("div", { className: "feed-body" })
		: observationData
			? activeMode === "facts"
				? renderFactsContent(observationData.facts) || h("div", { className: bodyClassName })
				: renderNarrativeContent(
						activeMode === "narrative" ? observationData.narrative : observationData.summary,
						bodyClassName,
					) || h("div", { className: bodyClassName })
			: h("div", { className: "feed-body" });

	const kindChipLabel = displayKindValue.replaceAll("_", " ");
	const filesRow = files.length
		? h(
				"div",
				{ className: "feed-files" },
				files.map((file, index) =>
					h("span", { className: "feed-file", key: `${String(file)}-${index}` }, String(file)),
				),
			)
		: null;
	return h(
		"article",
		{
			className: `feed-item ${displayKindValue}${isNew ? " new-item" : ""}`.trim(),
			"data-key": rowKey,
		},
		h(
			"div",
			{ className: "feed-kind-banner" },
			h(Chip, { variant: "kind", tone: displayKindValue }, kindChipLabel),
		),
		h(
			"div",
			{ className: "feed-card-body" },
			h(
				"div",
				{ className: "feed-card-header" },
				h(
					"div",
					{ className: "feed-header" },
					h("div", {
						className: "feed-title title",
						dangerouslySetInnerHTML: { __html: highlightText(displayTitle, state.feedQuery) },
					}),
					h("div", { className: "feed-card-subtitle small" }, secondaryMeta),
				),
				h(
					"div",
					{ className: "feed-actions" },
					observationData
						? h(FeedViewToggle, {
								active: activeMode,
								modes,
								onSelect: (mode) => setActiveMode(mode),
							})
						: null,
					h(
						Tooltip,
						{ label: formatDate(createdAtRaw), side: "left" },
						h("div", { className: "small feed-age" }, relative),
					),
				),
			),
			h(
				"div",
				{ className: "feed-provenance" },
				h(ProvenanceChip, { label: actor, variant: actor === "You" ? "mine" : "author" }),
				h(ProvenanceChip, { label: visibility || "private", variant: visibility || "private" }),
				memoryIdLabel
					? h(
							Tooltip,
							{ label: `Memory database id ${memoryId}`, side: "top" },
							h(ProvenanceChip, { label: memoryIdLabel, variant: "memory-id" }),
						)
					: null,
			),
			h(
				"div",
				{ className: "feed-meta" },
				metaText || "No tags, files, or provenance details attached.",
			),
			bodyContent,
			h(
				"div",
				{ className: "feed-footer" },
				h(
					"div",
					{ className: "feed-footer-left" },
					tags.length
						? h(
								"div",
								{ className: "feed-tags" },
								tags.map((tag, index) => h(TagChip, { key: `${String(tag)}-${index}`, tag })),
							)
						: null,
				),
				h(
					"div",
					{ className: "feed-footer-right" },
					canClamp
						? h(
								"button",
								{
									className: "feed-expand",
									onClick: () => {
										const nextValue = !expanded;
										state.itemExpandState.set(activeExpandKey, nextValue);
										setExpanded(nextValue);
									},
									type: "button",
								},
								expanded ? "Collapse" : "Expand",
							)
						: null,
				),
			),
			filesRow,
		),
	);
}
