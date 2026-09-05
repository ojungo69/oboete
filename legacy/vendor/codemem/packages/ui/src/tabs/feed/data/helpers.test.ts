import { describe, expect, it } from "vitest";
import { trustStateLabel } from "./helpers";

describe("trustStateLabel", () => {
	it("spells out the two states that have a dedicated wording", () => {
		expect(trustStateLabel("legacy_unknown")).toBe("legacy provenance");
		expect(trustStateLabel("unreviewed")).toBe("unreviewed");
	});

	it("falls back to the raw state with underscores turned into spaces", () => {
		expect(trustStateLabel("self_reported_trusted")).toBe("self reported trusted");
		expect(trustStateLabel("trusted")).toBe("trusted");
	});
});
