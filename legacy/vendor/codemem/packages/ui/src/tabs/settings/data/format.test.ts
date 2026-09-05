import { describe, expect, it } from "vitest";
import { formatSettingsKey, joinPhrases } from "./format";

describe("settings formatting helpers", () => {
	it("turns snake_case keys into spaced labels and tolerates an empty key", () => {
		expect(formatSettingsKey("observer_runtime")).toBe("observer runtime");
		expect(formatSettingsKey("")).toBe("");
	});

	it("joins phrases with an Oxford comma once there are three or more", () => {
		expect(joinPhrases([])).toBe("");
		expect(joinPhrases(["API key"])).toBe("API key");
		expect(joinPhrases(["API key", "env var"])).toBe("API key and env var");
		expect(joinPhrases(["API key", "env var", "keychain"])).toBe("API key, env var, and keychain");
	});
});
