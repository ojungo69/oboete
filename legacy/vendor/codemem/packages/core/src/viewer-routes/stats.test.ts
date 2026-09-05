import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { MemoryStore } from "../store.js";
import { openTestMemoryStore } from "../test-utils.js";
import { statsRoutes } from "./stats.js";

const REPOSITORY_A = `repo-v1:sha256:${"a".repeat(64)}`;
const REPOSITORY_B = `repo-v1:sha256:${"b".repeat(64)}`;

describe("viewer stats privacy boundary", () => {
	let dir: string;
	let store: MemoryStore;
	let eligibleMemoryIds: number[];

	beforeEach(() => {
		dir = mkdtempSync(join(tmpdir(), "codemem-viewer-stats-"));
		store = openTestMemoryStore(join(dir, "memory.sqlite"));
		eligibleMemoryIds = [];
		for (const [index, fixture] of [
			["eligible-same", "eligible", REPOSITORY_A],
			["eligible-unknown", "eligible", null],
			["local-only-same", "local_only", REPOSITORY_A],
			["private-cross", "private", REPOSITORY_B],
			["secret-unknown", "secret", null],
		].entries()) {
			const [name, sensitivity, repositoryIdentity] = fixture;
			const now = `2026-09-01T00:00:0${index}.000Z`;
			const session = store.db
				.prepare(
					`INSERT INTO sessions(started_at, project, repository_identity)
					 VALUES (?, ?, ?)`,
				)
				.run(now, name, repositoryIdentity);
			const sessionId = Number(session.lastInsertRowid);
			const memory = store.db
				.prepare(
					`INSERT INTO memory_items(
						session_id, kind, title, body_text, active, created_at, updated_at,
						metadata_json, sensitivity, repository_identity
					 ) VALUES (?, 'discovery', ?, ?, 1, ?, ?, '{}', ?, ?)`,
				)
				.run(
					sessionId,
					`${name}-title-sentinel`,
					`${name}-body-sentinel`,
					now,
					now,
					sensitivity,
					repositoryIdentity,
				);
			const memoryId = Number(memory.lastInsertRowid);
			if (sensitivity === "eligible") eligibleMemoryIds.push(memoryId);
			store.db
				.prepare(
					`INSERT INTO artifacts(
						session_id, kind, content_text, created_at, metadata_json,
						sensitivity, repository_identity
					 ) VALUES (?, 'note', ?, ?, '{}', ?, ?)`,
				)
				.run(sessionId, `${name}-artifact-sentinel`, now, sensitivity, repositoryIdentity);
			store.db
				.prepare(
					`INSERT INTO usage_events(
						session_id, event, tokens_read, tokens_written, tokens_saved,
						created_at, metadata_json
					 ) VALUES (?, 'pack', ?, 1, 2, ?, ?)`,
				)
				.run(
					sessionId,
					(index + 1) * 10,
					now,
					JSON.stringify({
						pack_item_ids: [memoryId],
						sentinel: `${name}-usage-sentinel`,
					}),
				);
			store.recordRawEvent({
				opencodeSessionId: name,
				eventId: `${name}-event`,
				eventType: "message",
				payload: { sentinel: `${name}-raw-sentinel` },
				repositoryIdentity,
				sensitivity,
			});
		}
	});

	afterEach(() => {
		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	it("reports eligible-only database and usage totals", async () => {
		const routes = statsRoutes(() => store);
		const response = await routes.request("/api/stats");
		const body = (await response.json()) as {
			database: Record<string, number>;
			usage: { totals: Record<string, number> };
		};

		expect(response.status).toBe(200);
		expect(body.database).toMatchObject({
			sessions: 2,
			memory_items: 2,
			active_memory_items: 2,
			artifacts: 2,
			raw_events: 2,
		});
		expect(body.usage.totals).toMatchObject({
			events: 2,
			tokens_read: 30,
			tokens_written: 2,
			tokens_saved: 4,
		});
	});

	it("gates the artifact total by session visibility like the artifact route", async () => {
		const routes = statsRoutes(() => store);
		const artifactsTotal = async () =>
			((await (await routes.request("/api/stats")).json()) as { database: { artifacts: number } })
				.database.artifacts;
		const now = "2026-09-01T00:00:50.000Z";
		const insertSession = (name: string, sensitivities: string[]) => {
			const sessionId = Number(
				store.db
					.prepare(
						"INSERT INTO sessions(started_at, project, repository_identity) VALUES (?, ?, NULL)",
					)
					.run(now, name).lastInsertRowid,
			);
			for (const sensitivity of sensitivities) {
				store.db
					.prepare(
						`INSERT INTO memory_items(
							session_id, kind, title, body_text, active, created_at, updated_at,
							metadata_json, sensitivity, repository_identity
						 ) VALUES (?, 'discovery', ?, 'body', 1, ?, ?, '{}', ?, NULL)`,
					)
					.run(sessionId, `${name}-${sensitivity}`, now, now, sensitivity);
			}
			store.db
				.prepare(
					`INSERT INTO artifacts(
						session_id, kind, content_text, created_at, metadata_json,
						sensitivity, repository_identity
					 ) VALUES (?, 'note', ?, ?, '{}', 'eligible', NULL)`,
				)
				.run(sessionId, `${name}-artifact`, now);
		};

		expect(await artifactsTotal()).toBe(2);
		// Gate fires: an eligible artifact in a session that also holds a
		// restricted active memory is not counted (the artifact route 404s it).
		insertSession("mixed", ["eligible", "secret"]);
		expect(await artifactsTotal()).toBe(2);
		// Gate passes: an all-visible session's artifact is counted.
		insertSession("clean", ["eligible"]);
		expect(await artifactsTotal()).toBe(3);
	});

	it("filters usage aggregates, rows, and referenced memory ids before serialization", async () => {
		const routes = statsRoutes(() => store);
		const response = await routes.request("/api/usage");
		const body = (await response.json()) as {
			totals: Record<string, number>;
			totals_global: Record<string, number>;
			recent_packs: Array<{ metadata_json: { pack_item_ids: number[] } }>;
		};

		expect(response.status).toBe(200);
		expect(body.totals).toEqual({
			tokens_read: 30,
			tokens_written: 2,
			tokens_saved: 4,
			count: 2,
		});
		expect(body.totals_global).toEqual(body.totals);
		expect(body.recent_packs).toHaveLength(2);
		expect(
			body.recent_packs.flatMap((row) => row.metadata_json.pack_item_ids).sort((a, b) => a - b),
		).toEqual([...eligibleMemoryIds].sort((a, b) => a - b));
		const serialized = JSON.stringify(body);
		expect(serialized).not.toContain("local-only-same");
		expect(serialized).not.toContain("private-cross");
		expect(serialized).not.toContain("secret-unknown");
	});
});
