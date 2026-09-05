import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { MemoryStore } from "../store.js";
import { openTestMemoryStore } from "../test-utils.js";
import type { SensitivityV1 } from "../types.js";
import { rawEventReadRoutes } from "./raw-events.js";

const REPOSITORY_A = `repo-v1:sha256:${"a".repeat(64)}`;
const REPOSITORY_B = `repo-v1:sha256:${"b".repeat(64)}`;

describe("viewer raw-event privacy boundary", () => {
	let dir: string;
	let store: MemoryStore;

	beforeEach(() => {
		dir = mkdtempSync(join(tmpdir(), "codemem-viewer-raw-events-"));
		store = openTestMemoryStore(join(dir, "memory.sqlite"));
		for (const fixture of [
			["eligible-same", "eligible", REPOSITORY_A],
			["eligible-unknown", "eligible", null],
			["local-only-same", "local_only", REPOSITORY_A],
			["private-cross", "private", REPOSITORY_B],
			["secret-unknown", "secret", null],
		] as const) {
			const [streamId, sensitivity, repositoryIdentity] = fixture;
			store.recordRawEvent({
				opencodeSessionId: streamId,
				eventId: `${streamId}-event`,
				eventType: "message",
				payload: { sentinel: `${streamId}-payload-sentinel` },
				repositoryIdentity,
				sensitivity: sensitivity as SensitivityV1,
			});
			store.updateRawEventSessionMeta({
				opencodeSessionId: streamId,
				cwd: `/restricted/${streamId}-cwd-sentinel`,
				project: `${streamId}-project-sentinel`,
			});
		}
	});

	afterEach(() => {
		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	it("counts only eligible pending rows when the viewer has no verified repository", async () => {
		const routes = rawEventReadRoutes(() => store);
		const response = await routes.request("/api/raw-events");

		expect(response.status).toBe(200);
		expect(await response.json()).toEqual({ pending: 2, sessions: 2 });
	});

	it("returns only eligible safe status rows without cwd, project, or restricted sentinels", async () => {
		const routes = rawEventReadRoutes(() => store);
		const response = await routes.request("/api/raw-events/status");
		const body = (await response.json()) as {
			items: Array<Record<string, unknown>>;
			totals: { pending: number; sessions: number };
		};

		expect(response.status).toBe(200);
		expect(body.totals).toEqual({ pending: 2, sessions: 2 });
		expect(body.items.map((item) => item.stream_id).sort()).toEqual([
			"eligible-same",
			"eligible-unknown",
		]);
		for (const item of body.items) {
			expect(item).not.toHaveProperty("cwd");
			expect(item).not.toHaveProperty("project");
		}
		const serialized = JSON.stringify(body);
		expect(serialized).not.toContain("local-only-same");
		expect(serialized).not.toContain("private-cross");
		expect(serialized).not.toContain("secret-unknown");
		expect(serialized).not.toContain("cwd-sentinel");
		expect(serialized).not.toContain("payload-sentinel");
	});
});
