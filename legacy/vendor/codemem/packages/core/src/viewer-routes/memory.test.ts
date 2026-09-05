import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileRunnerLocalDestinationBoundary,
} from "../destination-boundary.js";
import type { MemoryStore } from "../store.js";
import { openTestMemoryStore } from "../test-utils.js";
import { memoryRoutes } from "./memory.js";

const REPOSITORY_A = `repo-v1:sha256:${"a".repeat(64)}`;
const REPOSITORY_B = `repo-v1:sha256:${"b".repeat(64)}`;

type Fixture = {
	name: string;
	sensitivity: "eligible" | "local_only" | "private" | "secret";
	repositoryIdentity: string | null;
	sessionId: number;
};

describe("viewer memory privacy boundary", () => {
	let dir: string;
	let store: MemoryStore;
	let fixtures: Fixture[];

	beforeEach(() => {
		dir = mkdtempSync(join(tmpdir(), "codemem-viewer-memory-"));
		store = openTestMemoryStore(join(dir, "memory.sqlite"));
		fixtures = [
			{ name: "eligible-same", sensitivity: "eligible", repositoryIdentity: REPOSITORY_A },
			{ name: "eligible-unknown", sensitivity: "eligible", repositoryIdentity: null },
			{ name: "local-only-same", sensitivity: "local_only", repositoryIdentity: REPOSITORY_A },
			{ name: "private-cross", sensitivity: "private", repositoryIdentity: REPOSITORY_B },
			{ name: "secret-unknown", sensitivity: "secret", repositoryIdentity: null },
		].map((fixture, index) => {
			const now = `2026-09-01T00:00:0${index}.000Z`;
			const session = store.db
				.prepare(
					`INSERT INTO sessions(
						started_at, cwd, project, git_remote, git_branch, user,
						tool_version, metadata_json, repository_identity
					 ) VALUES (?, ?, ?, ?, ?, ?, 'test', ?, ?)`,
				)
				.run(
					now,
					`/${fixture.name}-cwd-sentinel`,
					fixture.name,
					`${fixture.name}-git-remote-sentinel`,
					`${fixture.name}-branch-sentinel`,
					`${fixture.name}-user-sentinel`,
					JSON.stringify({ sentinel: `${fixture.name}-metadata-sentinel` }),
					fixture.repositoryIdentity,
				);
			const sessionId = Number(session.lastInsertRowid);
			for (const [kind, suffix] of [
				["discovery", "observation"],
				["session_summary", "summary"],
			] as const) {
				store.db
					.prepare(
						`INSERT INTO memory_items(
							session_id, kind, title, body_text, active, created_at, updated_at,
							metadata_json, project, sensitivity, repository_identity
						 ) VALUES (?, ?, ?, ?, 1, ?, ?, '{}', ?, ?, ?)`,
					)
					.run(
						sessionId,
						kind,
						`${fixture.name}-${suffix}-title`,
						`${fixture.name}-${suffix}-body-sentinel`,
						now,
						now,
						fixture.name,
						fixture.sensitivity,
						fixture.repositoryIdentity,
					);
			}
			store.db
				.prepare(
					`INSERT INTO user_prompts(
						session_id, project, prompt_text, prompt_number, created_at,
						created_at_epoch, metadata_json, sensitivity, repository_identity
					 ) VALUES (?, ?, ?, 1, ?, ?, '{}', ?, ?)`,
				)
				.run(
					sessionId,
					fixture.name,
					`${fixture.name}-prompt-sentinel`,
					now,
					index,
					fixture.sensitivity,
					fixture.repositoryIdentity,
				);
			store.db
				.prepare(
					`INSERT INTO artifacts(
						session_id, kind, path, content_text, created_at, metadata_json,
						sensitivity, repository_identity
					 ) VALUES (?, 'note', ?, ?, ?, '{}', ?, ?)`,
				)
				.run(
					sessionId,
					`${fixture.name}-artifact-path-sentinel`,
					`${fixture.name}-artifact-body-sentinel`,
					now,
					fixture.sensitivity,
					fixture.repositoryIdentity,
				);
			return { ...fixture, sessionId };
		});
		const eligibleParent = fixtures.find((fixture) => fixture.name === "eligible-same");
		if (!eligibleParent) throw new Error("eligible viewer fixture missing");
		store.db
			.prepare(
				`INSERT INTO user_prompts(
					session_id, project, prompt_text, prompt_number, created_at,
					created_at_epoch, metadata_json, sensitivity, repository_identity
				 ) VALUES (?, ?, 'eligible-parent-secret-prompt-sentinel', 2, ?, 10, '{}', 'secret', ?)`,
			)
			.run(
				eligibleParent.sessionId,
				eligibleParent.name,
				"2026-09-01T00:00:10.000Z",
				eligibleParent.repositoryIdentity,
			);
		store.db
			.prepare(
				`INSERT INTO artifacts(
					session_id, kind, path, content_text, created_at, metadata_json,
					sensitivity, repository_identity
				 ) VALUES (?, 'note', 'eligible-parent-secret-path-sentinel',
					'eligible-parent-secret-artifact-sentinel', ?, '{}', 'secret', ?)`,
			)
			.run(eligibleParent.sessionId, "2026-09-01T00:00:10.000Z", eligibleParent.repositoryIdentity);
	});

	afterEach(() => {
		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	it("returns only eligible observations, summaries, and recent memories without cwd", async () => {
		const routes = memoryRoutes(() => store);
		const responses = await Promise.all(
			["/api/observations", "/api/summaries", "/api/memory"].map(async (path) => {
				const response = await routes.request(path);
				return [path, response.status, await response.json()] as const;
			}),
		);

		for (const [path, status, rawBody] of responses) {
			const body = rawBody as { items: Array<Record<string, unknown>> };
			expect(status, path).toBe(200);
			expect(body.items).toHaveLength(path === "/api/memory" ? 4 : 2);
			expect(
				body.items.every((item) => item.sensitivity === "eligible"),
				path,
			).toBe(true);
			for (const item of body.items) expect(item).not.toHaveProperty("cwd");
			const serialized = JSON.stringify(body);
			expect(serialized, path).not.toContain("local-only-same");
			expect(serialized, path).not.toContain("private-cross");
			expect(serialized, path).not.toContain("secret-unknown");
			expect(serialized, path).not.toContain("cwd-sentinel");
		}
	});

	it("returns eligible projects, prompt/artifact counts, and safe session shells", async () => {
		const routes = memoryRoutes(() => store);
		const projects = (await (await routes.request("/api/projects")).json()) as {
			projects: string[];
		};
		const counts = (await (await routes.request("/api/session")).json()) as Record<string, number>;
		const sessions = (await (await routes.request("/api/sessions")).json()) as {
			items: Array<Record<string, unknown>>;
		};

		expect(projects.projects).toEqual(["eligible-same", "eligible-unknown"]);
		expect(counts).toEqual({
			total: 8,
			memories: 4,
			artifacts: 2,
			prompts: 2,
			observations: 2,
		});
		expect(sessions.items.map((item) => item.project).sort()).toEqual([
			"eligible-same",
			"eligible-unknown",
		]);
		for (const item of sessions.items) {
			for (const field of ["cwd", "git_remote", "git_branch", "user", "metadata_json"]) {
				expect(item).not.toHaveProperty(field);
			}
		}
		const serialized = JSON.stringify({ projects, counts, sessions });
		expect(serialized).not.toContain("local-only-same");
		expect(serialized).not.toContain("private-cross");
		expect(serialized).not.toContain("secret-unknown");
		expect(serialized).not.toContain("metadata-sentinel");
	});

	it("keeps prompts/artifacts of sessions without visible memories out of the counts", async () => {
		const routes = memoryRoutes(() => store);
		const before = (await (await routes.request("/api/session")).json()) as Record<string, number>;

		// A session whose prompt and artifact rows are eligible, but whose only
		// memory is secret: the ownership EXISTS gate must keep its rows out of
		// the counts even though the row-level sensitivity predicate passes.
		const now = "2026-09-01T00:00:20.000Z";
		const session = store.db
			.prepare(
				`INSERT INTO sessions(
					started_at, cwd, project, git_remote, git_branch, user,
					tool_version, metadata_json, repository_identity
				 ) VALUES (?, '/orphan-cwd', 'orphan-project', 'r', 'b', 'u', 'test', '{}', NULL)`,
			)
			.run(now);
		const sessionId = Number(session.lastInsertRowid);
		store.db
			.prepare(
				`INSERT INTO memory_items(
					session_id, kind, title, body_text, active, created_at, updated_at,
					metadata_json, project, sensitivity, repository_identity
				 ) VALUES (?, 'discovery', 't', 'orphan-body', 1, ?, ?, '{}', 'orphan-project', 'secret', NULL)`,
			)
			.run(sessionId, now, now);
		store.db
			.prepare(
				`INSERT INTO user_prompts(
					session_id, project, prompt_text, prompt_number, created_at,
					created_at_epoch, metadata_json, sensitivity, repository_identity
				 ) VALUES (?, 'orphan-project', 'orphan-prompt', 1, ?, 20, '{}', 'eligible', NULL)`,
			)
			.run(sessionId, now);
		store.db
			.prepare(
				`INSERT INTO artifacts(
					session_id, kind, path, content_text, created_at, metadata_json,
					sensitivity, repository_identity
				 ) VALUES (?, 'note', 'orphan-path', 'orphan-artifact', ?, '{}', 'eligible', NULL)`,
			)
			.run(sessionId, now);

		const after = (await (await routes.request("/api/session")).json()) as Record<string, number>;

		// Gate fires: nothing from the orphan session is counted.
		// Gate passes: the eligible sessions' counts are unchanged.
		expect(after).toEqual(before);
		expect(after.prompts).toBe(2);
		expect(after.artifacts).toBe(2);
	});

	it("keeps mixed-visibility sessions' artifacts out of the counts, matching route access", async () => {
		const routes = memoryRoutes(() => store);
		const before = (await (await routes.request("/api/session")).json()) as Record<string, number>;

		// A session with one visible memory AND one restricted memory: the
		// /api/artifacts route 404s it (sessionAllowsArtifactAccess requires
		// every active memory visible), so the count must exclude it too.
		const now = "2026-09-01T00:00:30.000Z";
		const session = store.db
			.prepare(
				`INSERT INTO sessions(
					started_at, cwd, project, git_remote, git_branch, user,
					tool_version, metadata_json, repository_identity
				 ) VALUES (?, '/mixed-cwd', 'mixed-project', 'r', 'b', 'u', 'test', '{}', NULL)`,
			)
			.run(now);
		const sessionId = Number(session.lastInsertRowid);
		const insertMemory = (sensitivity: string) =>
			store.db
				.prepare(
					`INSERT INTO memory_items(
						session_id, kind, title, body_text, active, created_at, updated_at,
						metadata_json, project, sensitivity, repository_identity
					 ) VALUES (?, 'discovery', 't', 'mixed-body', 1, ?, ?, '{}', 'mixed-project', ?, NULL)`,
				)
				.run(sessionId, now, now, sensitivity);
		insertMemory("eligible");
		insertMemory("secret");
		store.db
			.prepare(
				`INSERT INTO artifacts(
					session_id, kind, path, content_text, created_at, metadata_json,
					sensitivity, repository_identity
				 ) VALUES (?, 'note', 'mixed-path', 'mixed-artifact', ?, '{}', 'eligible', NULL)`,
			)
			.run(sessionId, now);

		const after = (await (await routes.request("/api/session")).json()) as Record<string, number>;
		const artifactsResponse = await routes.request(`/api/artifacts?session_id=${sessionId}`);

		// Gate fires: count and route agree — neither exposes the mixed session.
		expect(after.artifacts).toBe(before.artifacts);
		expect(artifactsResponse.status).toBe(404);
	});

	it("hides mixed sessions from counts under a verified local boundary despite NULL-identity rows", async () => {
		// Under a verified local boundary the restricted predicate compares
		// repository_identity — for a legacy local_only row with NULL identity
		// the comparison is SQL NULL, and a bare NOT would silently skip it.
		const boundary = compileRunnerLocalDestinationBoundary({
			consumer: "viewer",
			configurationFingerprint: CAPTURE_ONLY_DESTINATION_FINGERPRINT,
			repositoryIdentity: REPOSITORY_A,
		});
		const routes = memoryRoutes(() => store, boundary);
		const before = (await (await routes.request("/api/session")).json()) as Record<string, number>;

		const now = "2026-09-01T00:00:40.000Z";
		const session = store.db
			.prepare(
				`INSERT INTO sessions(
					started_at, cwd, project, git_remote, git_branch, user,
					tool_version, metadata_json, repository_identity
				 ) VALUES (?, '/legacy-cwd', 'legacy-project', 'r', 'b', 'u', 'test', '{}', ?)`,
			)
			.run(now, REPOSITORY_A);
		const sessionId = Number(session.lastInsertRowid);
		const insertMemory = (sensitivity: string, repositoryIdentity: string | null) =>
			store.db
				.prepare(
					`INSERT INTO memory_items(
						session_id, kind, title, body_text, active, created_at, updated_at,
						metadata_json, project, sensitivity, repository_identity
					 ) VALUES (?, 'discovery', 't', 'legacy-body', 1, ?, ?, '{}', 'legacy-project', ?, ?)`,
				)
				.run(sessionId, now, now, sensitivity, repositoryIdentity);
		insertMemory("eligible", REPOSITORY_A);
		insertMemory("local_only", null);
		store.db
			.prepare(
				`INSERT INTO artifacts(
					session_id, kind, path, content_text, created_at, metadata_json,
					sensitivity, repository_identity
				 ) VALUES (?, 'note', 'legacy-path', 'legacy-artifact', ?, '{}', 'eligible', ?)`,
			)
			.run(sessionId, now, REPOSITORY_A);

		const after = (await (await routes.request("/api/session")).json()) as Record<string, number>;
		const artifactsResponse = await routes.request(`/api/artifacts?session_id=${sessionId}`);

		expect(after.artifacts).toBe(before.artifacts);
		expect(artifactsResponse.status).toBe(404);
	});

	it("returns eligible artifacts and hides sessions whose content is all restricted", async () => {
		const routes = memoryRoutes(() => store);
		const eligible = fixtures.find((fixture) => fixture.name === "eligible-same");
		const restricted = fixtures.find((fixture) => fixture.name === "private-cross");
		if (!eligible || !restricted) throw new Error("viewer fixtures missing");

		const eligibleResponse = await routes.request(
			`/api/artifacts?session_id=${eligible.sessionId}`,
		);
		const eligibleBody = (await eligibleResponse.json()) as {
			items: Array<Record<string, unknown>>;
		};
		const restrictedResponse = await routes.request(
			`/api/artifacts?session_id=${restricted.sessionId}`,
		);

		expect(eligibleResponse.status).toBe(200);
		expect(eligibleBody.items).toHaveLength(1);
		expect(eligibleBody.items[0]).toMatchObject({ sensitivity: "eligible" });
		expect(JSON.stringify(eligibleBody)).not.toContain("eligible-parent-secret");
		expect(restrictedResponse.status).toBe(404);
		expect(JSON.stringify(await restrictedResponse.json())).not.toContain("private-cross");
	});
});
