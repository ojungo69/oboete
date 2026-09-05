import { describe, expect, it } from "vitest";
import { formatWhereHuman } from "./config.js";

describe("config command", () => {
	it("renders the selected config and fallback chain in precedence order", () => {
		expect(
			formatWhereHuman({
				resolved: {
					path: "cli.json",
					source: "cli-flag",
					reason: "selected by --config",
					exists: true,
					valid: true,
				},
				fallbackChain: [
					{
						path: "legacy.json",
						source: "legacy-global",
						reason: "lower precedence",
						exists: false,
						valid: true,
					},
				],
			}),
		).toBe(
			">>> [cli-flag] cli.json\n       selected by --config (exists)\n    [legacy-global] legacy.json\n       lower precedence (missing)",
		);
	});
});
