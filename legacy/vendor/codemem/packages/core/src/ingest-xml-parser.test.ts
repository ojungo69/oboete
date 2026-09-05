import { describe, expect, it } from "vitest";
import {
	parseObserverResponse,
	shouldPreferRepairedObserverResponse,
} from "./ingest-xml-parser.js";

type ClaimedItem = "observation" | "summary";

function claimedXml(item: ClaimedItem, citations: string, kind = "decision"): string {
	return item === "observation"
		? `<observation><type>${kind}</type><title>Keep the source</title>${citations}</observation>`
		: `<summary><request>Keep the source</request>${citations}</summary>`;
}

function claimedItem(raw: string, item: ClaimedItem) {
	const parsed = parseObserverResponse(raw, { requireCitations: true });
	return item === "observation" ? parsed.observations[0] : parsed.summary;
}

describe("parseObserverResponse claimed citations", () => {
	it.each([
		"observation",
		"summary",
	] as const)("parses canonical whole-event and explicit-span citations on %s", (item) => {
		const parsed = claimedItem(
			claimedXml(
				item,
				'<citations><cite source="0"/><cite source="2" start="1" end="4"/></citations>',
			),
			item,
		);

		expect(parsed?.citations).toEqual([
			{ source: 0, start: null, end: null },
			{ source: 2, start: 1, end: 4 },
		]);
	});

	it.each([
		["missing", ""],
		["empty", "<citations></citations>"],
		[
			"multiple direct blocks",
			'<citations><cite source="0"/></citations><citations><cite source="1"/></citations>',
		],
		[
			"nested block",
			'<files_read><file><citations><cite source="0"/></citations></file></files_read>',
		],
		["non-self-closing cite", '<citations><cite source="0"></cite></citations>'],
		["duplicate ordinal", '<citations><cite source="0"/><cite source="0"/></citations>'],
		["non-increasing ordinals", '<citations><cite source="2"/><cite source="1"/></citations>'],
		["noncanonical ordinal", '<citations><cite source="01"/></citations>'],
		["one-sided span", '<citations><cite source="0" start="1"/></citations>'],
		["malformed span", '<citations><cite source="0" start="one" end="2"/></citations>'],
		["eventId authority", '<citations><cite source="0" eventId="42"/></citations>'],
		["repository authority", '<citations><cite source="0" repository="repo-a"/></citations>'],
		["digest authority", '<citations><cite source="0" digest="sha256:x"/></citations>'],
	] as const)("rejects %s for every claimed item", (_label, citations) => {
		for (const item of ["observation", "summary"] as const) {
			expect(claimedItem(claimedXml(item, citations), item)).toBeFalsy();
		}
	});

	it("keeps citation-free legacy parsing readable by default", () => {
		const parsed = parseObserverResponse(claimedXml("observation", ""));

		expect(parsed.observations).toHaveLength(1);
		expect(parsed.observations[0]?.citations).toEqual([]);
	});

	it("does not prefer repairs that drop or change claimed citations", () => {
		const initialRaw = claimedXml(
			"observation",
			'<citations><cite source="0"/></citations>',
			"insight",
		);
		const initial = parseObserverResponse(initialRaw, { requireCitations: true });

		for (const repairedRaw of [
			claimedXml("observation", "", "discovery"),
			claimedXml("observation", '<citations><cite source="1"/></citations>', "discovery"),
		]) {
			expect(
				shouldPreferRepairedObserverResponse(
					initial,
					repairedRaw,
					parseObserverResponse(repairedRaw, { requireCitations: true }),
					initialRaw,
				),
			).toBe(false);
		}
	});
});
