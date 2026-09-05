import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
	projectBasename,
	projectClause,
	projectMatchesFilter,
	resolveProject,
	resolveProjectRoot,
	resolveRepositoryIdentity,
} from "./project.js";

function git(cwd: string, ...args: string[]): void {
	execFileSync("git", ["-C", cwd, ...args], { stdio: "ignore" });
}

function repositoryIdentity(kind: "anchor" | "remote", value: string): string {
	const digest = createHash("sha256")
		.update(`free-mem:repository-${kind}:v1\0${value}`)
		.digest("hex");
	return `repo-v1:sha256:${digest}`;
}

describe("project helpers", () => {
	let tmpDir: string | null = null;

	afterEach(() => {
		if (tmpDir) {
			rmSync(tmpDir, { recursive: true, force: true });
			tmpDir = null;
		}
	});

	it("uses basename-aware SQL matching for project filters", () => {
		expect(projectClause("/Users/adam/workspace/codemem")).toEqual({
			clause:
				"(sessions.project = ? OR sessions.project LIKE ? ESCAPE '\\' OR sessions.project LIKE ? ESCAPE '\\')",
			params: ["codemem", "%/codemem", "%\\codemem"],
		});
	});

	it("escapes SQL LIKE wildcards in project filters", () => {
		expect(projectClause("weird_%project")).toEqual({
			clause:
				"(sessions.project = ? OR sessions.project LIKE ? ESCAPE '\\' OR sessions.project LIKE ? ESCAPE '\\')",
			params: ["weird_%project", "%/weird\\_\\%project", "%\\weird\\_\\%project"],
		});
	});

	it("matches exact and suffix project paths like Python", () => {
		expect(projectMatchesFilter("codemem", "codemem")).toBe(true);
		expect(projectMatchesFilter("/Users/adam/workspace/codemem", "codemem")).toBe(true);
		expect(projectMatchesFilter("codemem", "workspace/codemem")).toBe(true);
		expect(projectMatchesFilter("codemem", "workspace/other")).toBe(false);
		expect(projectMatchesFilter("codemem", null)).toBe(false);
	});

	it("resolves git repo basename as project", () => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		const repoRoot = join(tmpDir, "my-repo");
		const nested = join(repoRoot, "packages", "core");
		mkdirSync(join(repoRoot, ".git"), { recursive: true });
		writeFileSync(join(repoRoot, ".git", "HEAD"), "ref: refs/heads/main\n");
		mkdirSync(nested, { recursive: true });

		expect(resolveProject(nested)).toBe("my-repo");
	});

	it("ignores a .git directory that is not a Git repository", () => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		const falseRoot = join(tmpDir, "not-a-repo");
		const nested = join(falseRoot, "nested");
		mkdirSync(join(falseRoot, ".git"), { recursive: true });
		mkdirSync(nested, { recursive: true });

		expect(resolveProject(nested)).toBe("nested");
	});

	it("resolves main repo basename for git worktrees", () => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		const mainRepo = join(tmpDir, "main-repo");
		const worktree = join(tmpDir, "feature-worktree");
		mkdirSync(join(mainRepo, ".git", "worktrees", "feature-worktree"), { recursive: true });
		mkdirSync(worktree, { recursive: true });
		writeFileSync(
			join(worktree, ".git"),
			`gitdir: ${join(mainRepo, ".git", "worktrees", "feature-worktree")}`,
		);

		expect(resolveProject(worktree)).toBe("main-repo");
	});

	it("resolves the working-tree root from a subdirectory", () => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		const repoRoot = join(tmpDir, "my-repo");
		const nested = join(repoRoot, "packages", "core");
		mkdirSync(join(repoRoot, ".git"), { recursive: true });
		writeFileSync(join(repoRoot, ".git", "HEAD"), "ref: refs/heads/main\n");
		mkdirSync(nested, { recursive: true });

		expect(resolveProjectRoot(nested)).toBe(repoRoot);
	});

	it("resolves the linked worktree root, not the primary checkout", () => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		const mainRepo = join(tmpDir, "main-repo");
		const worktree = join(tmpDir, "feature-worktree");
		mkdirSync(join(mainRepo, ".git", "worktrees", "feature-worktree"), { recursive: true });
		mkdirSync(join(worktree, "packages"), { recursive: true });
		writeFileSync(
			join(worktree, ".git"),
			`gitdir: ${join(mainRepo, ".git", "worktrees", "feature-worktree")}`,
		);

		// resolveProject keeps the primary repo name, but the file root must be the
		// worktree itself so AGENTS.md is read from the worktree being mined.
		expect(resolveProject(worktree)).toBe("main-repo");
		expect(resolveProjectRoot(join(worktree, "packages"))).toBe(worktree);
	});

	it("honors explicit override before cwd resolution", () => {
		expect(resolveProject("/tmp/anything", " custom-project ")).toBe("custom-project");
	});

	it("returns cwd basename when no git repo exists", () => {
		expect(projectBasename("/tmp/foo/bar")).toBe("bar");
		expect(resolveProject("/tmp/foo/bar")).toBe("bar");
	});

	function createRepository(name = "repository"): string {
		tmpDir ??= mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		const repository = join(tmpDir, name);
		mkdirSync(repository);
		git(repository, "init", "--quiet");
		git(repository, "config", "user.email", "project-test@example.invalid");
		git(repository, "config", "user.name", "Project Test");
		writeFileSync(join(repository, "README.md"), "fixture\n");
		git(repository, "add", "README.md");
		git(repository, "commit", "--quiet", "-m", "fixture");
		return repository;
	}

	it("canonicalizes HTTPS identity and revalidates an origin change", () => {
		const repository = createRepository();
		git(repository, "remote", "add", "origin", "https://Git.Example.COM:443/acme//widgets.git/");

		const identityA = resolveRepositoryIdentity(repository);
		expect(identityA).toBe(
			repositoryIdentity("remote", "remote:https:git.example.com/acme/widgets"),
		);

		git(repository, "remote", "set-url", "origin", "https://git.example.com/acme/renamed.git");
		const identityB = resolveRepositoryIdentity(repository);
		expect(identityB).toBe(
			repositoryIdentity("remote", "remote:https:git.example.com/acme/renamed"),
		);
		expect(identityB).not.toBe(identityA);
	});

	it("keeps exact SSH usernames from colliding", () => {
		const repository = createRepository();
		git(repository, "remote", "add", "origin", "ssh://alice@Git.Example.COM:22/acme/widgets.git");
		const alice = resolveRepositoryIdentity(repository);
		expect(alice).toBe(
			repositoryIdentity("remote", "remote:ssh:alice@git.example.com/acme/widgets"),
		);

		git(repository, "remote", "set-url", "origin", "ssh://bob@git.example.com:22/acme/widgets.git");
		const bob = resolveRepositoryIdentity(repository);
		expect(bob).toBe(repositoryIdentity("remote", "remote:ssh:bob@git.example.com/acme/widgets"));
		expect(bob).not.toBe(alice);
	});

	it("keeps two repositories with the same basename distinct", () => {
		const first = createRepository("collide");
		tmpDir ??= mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		const nested = join(tmpDir, "elsewhere");
		mkdirSync(nested);
		const second = join(nested, "collide");
		mkdirSync(second);
		git(second, "init", "--quiet");

		const firstIdentity = resolveRepositoryIdentity(first);
		const secondIdentity = resolveRepositoryIdentity(second);
		expect(firstIdentity).toMatch(/^repo-v1:sha256:[a-f0-9]{64}$/);
		expect(secondIdentity).toMatch(/^repo-v1:sha256:[a-f0-9]{64}$/);
		expect(firstIdentity).not.toBe(secondIdentity);
	});

	it("falls back to the real common directory shared by linked worktrees", () => {
		const repository = createRepository("primary");
		const worktree = join(dirname(repository), "linked");
		git(repository, "remote", "add", "origin", "file:///tmp/not-authority.git");
		git(repository, "worktree", "add", "--quiet", "-b", "linked", worktree);
		const expected = repositoryIdentity("anchor", realpathSync(join(repository, ".git")));

		expect(resolveRepositoryIdentity(repository)).toBe(expected);
		expect(resolveRepositoryIdentity(worktree)).toBe(expected);
	});

	it("returns null outside a Git repository", () => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-project-test-"));
		expect(resolveRepositoryIdentity(tmpDir)).toBeNull();
	});
});
