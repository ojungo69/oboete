import { afterEach, describe, expect, it, vi } from "vitest";

describe("project-scope helpers", () => {
	afterEach(() => {
		delete process.env.CODEMEM_PROJECT;
		vi.resetModules();
		vi.unstubAllGlobals();
	});

	it("prefers CODEMEM_PROJECT when set", async () => {
		process.env.CODEMEM_PROJECT = "forced-project";
		const { resolveDefaultProject } = await import("./project-scope.js");
		expect(resolveDefaultProject()).toBe("forced-project");
	});

	it("buildFilters applies the default project when project is omitted", async () => {
		const { buildFilters } = await import("./project-scope.js");
		expect(buildFilters({ kind: "decision" }, "repo-name")).toEqual({
			kind: "decision",
			project: "repo-name",
		});
	});

	it("buildFilters respects an explicit project override", async () => {
		const { buildFilters } = await import("./project-scope.js");
		expect(buildFilters({ project: "manual", kind: "change" }, "repo-name")).toEqual({
			kind: "change",
			project: "manual",
		});
	});

	it("supports explicit null default project for future all-project contexts", async () => {
		const { buildFilters } = await import("./project-scope.js");
		expect(buildFilters({}, null)).toBeUndefined();
	});

	it("falls back to default project when env or request project is blank on read filters", async () => {
		process.env.CODEMEM_PROJECT = "   ";
		const cwd = vi.spyOn(process, "cwd").mockReturnValue("/tmp/codemem");
		try {
			const { buildFilters, resolveDefaultProject } = await import("./project-scope.js");
			expect(resolveDefaultProject()).toBe("codemem");
			expect(buildFilters({ project: "   ", kind: "change" }, "repo-name")).toEqual({
				kind: "change",
				project: "repo-name",
			});
		} finally {
			cwd.mockRestore();
		}
	});

	it("resolveWriteProject never falls back to the server default project", async () => {
		// Writes intentionally do not inherit cwd/server default. Otherwise blank
		// inputs in stdio mode would silently stamp a project the caller did not
		// ask for. See memory_remember in tools/items.ts.
		const { resolveWriteProject } = await import("./project-scope.js");
		expect(resolveWriteProject({ project: undefined, envProject: undefined })).toBeNull();
		expect(resolveWriteProject({ project: "   ", envProject: "   " })).toBeNull();
		expect(resolveWriteProject({ project: "explicit" })).toBe("explicit");
		expect(resolveWriteProject({ envProject: "env-value" })).toBe("env-value");
	});
});
