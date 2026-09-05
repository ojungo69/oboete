import { afterEach, describe, expect, it, vi } from "vitest";
import { distillCommand } from "./distill.js";

afterEach(() => {
	process.exitCode = 0;
	vi.restoreAllMocks();
});

describe("distill command", () => {
	it("registers shared and distill-specific options", () => {
		const options = distillCommand.options.map((option) => option.long);
		expect(options).toEqual(
			expect.arrayContaining([
				"--db-path",
				"--json",
				"--project",
				"--all-projects",
				"--limit",
				"--draft",
				"--apply",
			]),
		);
	});
});
