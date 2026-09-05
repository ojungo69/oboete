import { beforeEach, describe, expect, it } from "vitest";

import {
	ALL_TAB_IDS,
	getActiveTab,
	parseTabFromHash,
	resolveAccessibleTab,
	setActiveTab,
} from "./state";

describe("Viewer tab routing", () => {
	beforeEach(() => {
		localStorage.clear();
		window.location.hash = "";
	});

	it("exposes the canonical tab set", () => {
		expect(ALL_TAB_IDS).toEqual(["feed", "health"]);
	});

	it.each(["feed", "health"])("recognizes #%s as a canonical route", (tab) => {
		expect(parseTabFromHash(`#${tab}`)).toBe(tab);
	});

	it("falls back to feed for an unknown hash", () => {
		window.location.hash = "unknown";
		expect(getActiveTab()).toBe("feed");
	});

	it.each([
		"sync",
		"coordinator-admin",
		"projects",
		"sharing",
		"devices",
		"advanced",
	])("resolves removed route %s to feed", (removed) => {
		window.location.hash = removed;
		expect(getActiveTab()).toBe("feed");
	});

	it("resolves a removed saved tab to feed", () => {
		localStorage.setItem("codemem-tab", "sync");
		expect(getActiveTab()).toBe("feed");
	});

	it("writes canonical hashes for new navigation clicks", () => {
		window.location.hash = "feed";
		setActiveTab("health", { canonicalHash: true });

		expect(window.location.hash).toBe("#health");
		expect(localStorage.getItem("codemem-tab")).toBe("health");
	});

	it("keeps canonical tabs accessible and uses feed as the fallback", () => {
		expect(resolveAccessibleTab("health")).toBe("health");
		expect(resolveAccessibleTab("feed")).toBe("feed");
	});
});
