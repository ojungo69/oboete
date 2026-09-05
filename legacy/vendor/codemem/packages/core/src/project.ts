import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync, realpathSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";

const REPOSITORY_PROBE_BUDGET_MS = 100;
const REPOSITORY_PROBE_BUDGET_BYTES = 8 * 1024;
const SSH_USERNAME = /^[A-Za-z_][A-Za-z0-9._-]{0,63}$/;

function repositoryIdentity(domain: "anchor" | "remote", value: string): string {
	const digest = createHash("sha256")
		.update(`free-mem:repository-${domain}:v1\0${value}`, "utf8")
		.digest("hex");
	return `repo-v1:sha256:${digest}`;
}

function normalizedRemotePath(value: string): string | null {
	const parts = value.split("/").filter(Boolean);
	if (parts.length === 0) return null;
	const last = parts.at(-1) as string;
	parts[parts.length - 1] = last.endsWith(".git") ? last.slice(0, -4) : last;
	return parts.at(-1) ? parts.join("/") : null;
}

function canonicalRepositoryRemote(value: string): string | null {
	const raw = value.trim();
	if (!raw) return null;
	if (raw.startsWith("https://") || raw.startsWith("ssh://")) {
		let url: URL;
		try {
			url = new URL(raw);
		} catch {
			return null;
		}
		if (url.search || url.hash || !url.hostname) return null;
		const path = normalizedRemotePath(url.pathname);
		if (!path) return null;
		const host = url.hostname.toLowerCase();
		if (url.protocol === "https:") {
			if (url.username || url.password) return null;
			const authority = url.port && url.port !== "443" ? `${host}:${url.port}` : host;
			return `remote:https:${authority}/${path}`;
		}
		if (url.protocol !== "ssh:" || url.password || !SSH_USERNAME.test(url.username)) return null;
		const authority = url.port && url.port !== "22" ? `${host}:${url.port}` : host;
		return `remote:ssh:${url.username}@${authority}/${path}`;
	}

	const scp = /^([^@:/]+)@([^@:/]+):(.+)$/.exec(raw);
	if (!scp) return null;
	const username = scp[1] as string;
	const host = scp[2] as string;
	const path = normalizedRemotePath(scp[3] as string);
	if (!SSH_USERNAME.test(username) || !path) return null;
	return `remote:ssh:${username}@${host.toLowerCase()}/${path}`;
}

/** Resolve one trusted repository authority from the current Git state. */
export function resolveRepositoryIdentity(cwd: string): string | null {
	const deadline = Date.now() + REPOSITORY_PROBE_BUDGET_MS;
	let byteBudget = REPOSITORY_PROBE_BUDGET_BYTES;
	const probe = (args: string[]): string | null => {
		const timeout = deadline - Date.now();
		if (timeout <= 0 || byteBudget <= 0) return null;
		const result = spawnSync("git", args, {
			cwd,
			encoding: "utf8",
			maxBuffer: byteBudget,
			shell: false,
			timeout,
		});
		const stdout = typeof result.stdout === "string" ? result.stdout : "";
		const stderr = typeof result.stderr === "string" ? result.stderr : "";
		byteBudget -= Buffer.byteLength(stdout, "utf8") + Buffer.byteLength(stderr, "utf8");
		if (result.error || result.status !== 0 || byteBudget < 0) return null;
		return stdout.trim() || null;
	};

	const root = probe(["rev-parse", "--show-toplevel"]);
	const commonDir = root
		? probe(["rev-parse", "--path-format=absolute", "--git-common-dir"])
		: null;
	if (!root || !commonDir) return null;
	let realRoot: string;
	let realCommonDir: string;
	try {
		realRoot = realpathSync(root);
		realCommonDir = realpathSync(commonDir);
	} catch {
		return null;
	}
	if (!realRoot || !realCommonDir) return null;

	const remote = canonicalRepositoryRemote(probe(["remote", "get-url", "origin"]) ?? "");
	return remote
		? repositoryIdentity("remote", remote)
		: repositoryIdentity("anchor", realCommonDir);
}

export function projectBasename(value: string): string {
	let normalized = value.replaceAll("\\", "/");
	while (normalized.endsWith("/")) normalized = normalized.slice(0, -1);
	if (!normalized) return "";
	const parts = normalized.split("/");
	return parts.at(-1) ?? "";
}

function escapeSqlLikePattern(value: string): string {
	return value.replaceAll("\\", String.raw`\\`).replaceAll("%", "\\%").replaceAll("_", "\\_");
}

export function projectColumnClause(
	columnExpr: string,
	project: string,
): { clause: string; params: string[] } {
	const trimmed = project.trim();
	if (!trimmed) return { clause: "", params: [] };
	const value = /[\\/]/.test(trimmed) ? projectBasename(trimmed) : trimmed;
	if (!value) return { clause: "", params: [] };
	const escaped = escapeSqlLikePattern(value);
	return {
		clause: String.raw`(${columnExpr} = ? OR ${columnExpr} LIKE ? ESCAPE '\' OR ${columnExpr} LIKE ? ESCAPE '\')`,
		params: [value, `%/${escaped}`, `%\\${escaped}`],
	};
}

export function projectClause(project: string): { clause: string; params: string[] } {
	return projectColumnClause("sessions.project", project);
}

export function projectMatchesFilter(
	projectFilter: string | null | undefined,
	itemProject: string | null | undefined,
): boolean {
	if (!projectFilter) return true;
	if (!itemProject) return false;
	const normalizedFilter = projectFilter.trim().replaceAll("\\", "/");
	if (!normalizedFilter) return true;
	const filterValue = normalizedFilter.includes("/")
		? projectBasename(normalizedFilter)
		: normalizedFilter;
	const normalizedProject = itemProject.replaceAll("\\", "/");
	return normalizedProject === filterValue || normalizedProject.endsWith(`/${filterValue}`);
}

function findGitAnchor(startCwd: string): string | null {
	let current = resolve(startCwd);
	while (true) {
		const gitPath = resolve(current, ".git");
		if (existsSync(gitPath)) {
			try {
				if (lstatSync(gitPath).isDirectory()) {
					if (existsSync(resolve(gitPath, "HEAD"))) return current;
				} else {
					const text = readFileSync(gitPath, "utf8").trim();
					if (text.startsWith("gitdir:")) {
						const gitdir = resolve(current, text.slice("gitdir:".length).trim()).replaceAll(
							"\\",
							"/",
						);
						const worktreeMarker = "/.git/worktrees/";
						const worktreeIndex = gitdir.indexOf(worktreeMarker);
						if (worktreeIndex >= 0) {
							return gitdir.slice(0, worktreeIndex);
						}
					}
					return current;
				}
			} catch {
				return current;
			}
		}
		const parent = dirname(current);
		if (parent === current) return null;
		current = parent;
	}
}

export function resolveProject(cwd: string, override?: string | null): string | null {
	if (override != null) {
		const trimmed = override.trim();
		return trimmed || null;
	}
	const gitAnchor = findGitAnchor(cwd);
	if (gitAnchor) {
		return basename(gitAnchor);
	}
	return basename(resolve(cwd));
}

/**
 * Resolve the working-tree root for a directory by walking up to the nearest
 * `.git` marker and returning the directory that contains it. Returns null when
 * no repository is found.
 *
 * Unlike `resolveProject` (which follows a linked worktree's gitdir back to the
 * primary checkout for a stable project name), this returns the *current*
 * worktree root so repo-root files like AGENTS.md are read from the worktree
 * actually being used, not another checkout.
 */
export function resolveProjectRoot(cwd: string): string | null {
	let current = resolve(cwd);
	while (true) {
		if (existsSync(resolve(current, ".git"))) return current;
		const parent = dirname(current);
		if (parent === current) return null;
		current = parent;
	}
}
