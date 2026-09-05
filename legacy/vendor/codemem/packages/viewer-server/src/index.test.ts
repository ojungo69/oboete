/**
 * Viewer-server integration tests.
 *
 * Uses initTestSchema from @codemem/core (fix #5 — no duplicated DDL).
 * Uses Record<string, unknown> instead of Record<string, any> (fix #6).
 */

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { brotliCompressSync } from "node:zlib";
import type { MemoryStore } from "@codemem/core";
import {
	captureOnlyCapabilityProjection,
	initTestSchema,
	insertTestSession,
	startMaintenanceJob,
	updateMaintenanceJob,
	VERSION,
} from "@codemem/core";
import Database from "better-sqlite3";
import { describe, expect, it, vi } from "vitest";
import { openTestMemoryStore } from "../../core/src/test-utils.js";
import { createViewerReadHandler } from "../../core/src/viewer-read.js";
import { __usageCacheTestHooks } from "../../core/src/viewer-routes/stats.js";
import { createApp } from "./index.js";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

function createTestStore(seedDevice = true): { store: MemoryStore; cleanup: () => void } {
	const tmpDir = mkdtempSync(join(tmpdir(), "codemem-viewer-store-test-"));
	const dbPath = join(tmpDir, "test.sqlite");
	const rawDb = new Database(dbPath);
	initTestSchema(rawDb);
	if (seedDevice) {
		rawDb
			.prepare(
				"INSERT OR IGNORE INTO sync_device(device_id, public_key, fingerprint, created_at) VALUES (?, ?, ?, ?)",
			)
			.run("test-device-001", "test-public-key", "test-fingerprint", new Date().toISOString());
	}
	rawDb.close();
	const store = openTestMemoryStore(dbPath);
	return {
		store,
		cleanup: () => {
			store.close();
			rmSync(tmpDir, { recursive: true, force: true });
		},
	};
}

function insertTestMemory(
	store: MemoryStore,
	options: {
		sessionId: number;
		kind: string;
		title: string;
		bodyText?: string;
		metadata?: Record<string, unknown>;
		actorId?: string | null;
		originDeviceId?: string | null;
		createdAt?: string;
		active?: boolean;
		scopeId?: string | null;
	},
): number {
	const now = options.createdAt ?? new Date().toISOString();
	const result = store.db
		.prepare(
			`INSERT INTO memory_items (
				session_id, kind, title, subtitle, body_text, confidence, tags_text, active,
				created_at, updated_at, metadata_json, actor_id, actor_display_name, visibility,
				workspace_id, workspace_kind, origin_device_id, origin_source, trust_state,
				facts, narrative, concepts, files_read, files_modified, prompt_number, rev, import_key,
				scope_id, sensitivity
			) VALUES (?, ?, ?, NULL, ?, 0.5, '', ?, ?, ?, ?, ?, ?, 'shared', 'shared:default', 'shared', ?, ?, 'trusted', NULL, NULL, NULL, NULL, NULL, NULL, 1, ?, ?, 'eligible')`,
		)
		.run(
			options.sessionId,
			options.kind,
			options.title,
			options.bodyText ?? options.title,
			options.active === false ? 0 : 1,
			now,
			now,
			JSON.stringify(options.metadata ?? {}),
			options.actorId === undefined ? "local:test-device-001" : options.actorId,
			options.actorId == null || options.actorId === "local:test-device-001"
				? "Test User"
				: options.actorId,
			options.originDeviceId === undefined ? "test-device-001" : options.originDeviceId,
			String(options.metadata?.source ?? "test"),
			`${options.kind}-${options.title}-${now}`,
			options.scopeId ?? null,
		);
	return Number(result.lastInsertRowid);
}

/** Create a test Hono app backed by a fresh in-memory DB. */
function createTestApp(opts?: { seedDevice?: boolean; sweeper?: unknown }) {
	let store: MemoryStore | null = null;
	let storeCleanup: (() => void) | null = null;

	const storeFactory = () => {
		if (!store) {
			const created = createTestStore(opts?.seedDevice);
			store = created.store;
			storeCleanup = created.cleanup;
		}
		return store;
	};

	let readHandler: ReturnType<typeof createViewerReadHandler> | null = null;
	const rawApp = createApp({
		rpc: async (method, body = {}) => {
			if (method === "POST /v1/viewer/auth/verify") return { authenticated: true };
			if (method === "GET /v1/view") {
				readHandler ??= createViewerReadHandler({
					store: storeFactory(),
					sweeper: (opts?.sweeper ?? null) as never,
					capability: captureOnlyCapabilityProjection(),
				});
				return readHandler(body);
			}
			throw new Error(`unexpected viewer test RPC: ${method}`);
		},
	});
	const app = new Proxy(rawApp, {
		get(target, property, receiver) {
			if (property !== "request") return Reflect.get(target, property, receiver);
			return (input: RequestInfo | URL, init: RequestInit = {}) => {
				const headers = new Headers(init.headers);
				headers.set("Authorization", `Bearer ${"t".repeat(43)}`);
				return rawApp.request(input, { ...init, headers });
			};
		},
	});

	return {
		app,
		ensureStore: () => storeFactory(),
		getStore: () => store,
		cleanup: () => {
			storeCleanup?.();
			store = null;
			storeCleanup = null;
		},
	};
}

function grantSyncScopeToDevices(store: MemoryStore, scopeId: string, deviceIds: string[]): void {
	const now = "2026-01-01T00:00:00Z";
	store.db
		.prepare(
			`INSERT INTO replication_scopes(
				scope_id, label, kind, authority_type, membership_epoch, status, created_at, updated_at
			 ) VALUES (?, ?, 'team', 'coordinator', 1, 'active', ?, ?)
			 ON CONFLICT(scope_id) DO UPDATE SET updated_at = excluded.updated_at`,
		)
		.run(scopeId, scopeId, now, now);
	for (const deviceId of deviceIds) {
		store.db
			.prepare(
				`INSERT INTO scope_memberships(
					scope_id, device_id, role, status, membership_epoch, updated_at
				 ) VALUES (?, ?, 'member', 'active', 1, ?)
				 ON CONFLICT(scope_id, device_id) DO UPDATE SET updated_at = excluded.updated_at`,
			)
			.run(scopeId, deviceId, now);
	}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("viewer-server", () => {
	it("serves viewer shell and app bundle with cache-safe headers", async () => {
		const tmpDir = mkdtempSync(join(tmpdir(), "codemem-viewer-static-cache-"));
		const previousStaticDir = process.env.CODEMEM_VIEWER_STATIC_DIR;
		process.env.CODEMEM_VIEWER_STATIC_DIR = tmpDir;
		try {
			writeFileSync(
				join(tmpDir, "index.html"),
				'<!doctype html><script src="/assets/app.js"></script>',
			);
			writeFileSync(join(tmpDir, "app.js"), "globalThis.__codememTestApp = true;");
			const app = createApp();

			const index = await app.request("/");
			expect(index.headers.get("cache-control")).toBe("no-store");

			const bundle = await app.request("/assets/app.js");
			expect(bundle.status).toBe(200);
			expect(bundle.headers.get("cache-control")).toBe("no-cache");
		} finally {
			if (previousStaticDir == null) delete process.env.CODEMEM_VIEWER_STATIC_DIR;
			else process.env.CODEMEM_VIEWER_STATIC_DIR = previousStaticDir;
			rmSync(tmpDir, { recursive: true, force: true });
		}
	});

	it("serves the brotli-precompressed app bundle when the client accepts it", async () => {
		const tmpDir = mkdtempSync(join(tmpdir(), "codemem-viewer-static-br-"));
		const previousStaticDir = process.env.CODEMEM_VIEWER_STATIC_DIR;
		process.env.CODEMEM_VIEWER_STATIC_DIR = tmpDir;
		try {
			writeFileSync(
				join(tmpDir, "index.html"),
				'<!doctype html><script src="/assets/app.js"></script>',
			);
			const rawBundle = "globalThis.__codememTestApp = true;";
			writeFileSync(join(tmpDir, "app.js"), rawBundle);
			writeFileSync(join(tmpDir, "app.js.br"), brotliCompressSync(Buffer.from(rawBundle)));
			const app = createApp();

			const compressed = await app.request("/assets/app.js", {
				headers: { "Accept-Encoding": "br" },
			});
			expect(compressed.status).toBe(200);
			expect(compressed.headers.get("content-encoding")).toBe("br");

			// No matching encoding -> raw file, no Content-Encoding header.
			const identity = await app.request("/assets/app.js", {
				headers: { "Accept-Encoding": "identity" },
			});
			expect(identity.status).toBe(200);
			expect(identity.headers.get("content-encoding")).toBeNull();
		} finally {
			if (previousStaticDir == null) delete process.env.CODEMEM_VIEWER_STATIC_DIR;
			else process.env.CODEMEM_VIEWER_STATIC_DIR = previousStaticDir;
			rmSync(tmpDir, { recursive: true, force: true });
		}
	});

	it("createApp fails with a clear build hint when viewer assets are missing", () => {
		const tmpDir = mkdtempSync(join(tmpdir(), "codemem-viewer-static-missing-"));
		const previousStaticDir = process.env.CODEMEM_VIEWER_STATIC_DIR;
		process.env.CODEMEM_VIEWER_STATIC_DIR = tmpDir;
		try {
			expect(() => createApp()).toThrow(
				/Run `pnpm build` from the repo root before starting the viewer\./,
			);
		} finally {
			if (previousStaticDir == null) delete process.env.CODEMEM_VIEWER_STATIC_DIR;
			else process.env.CODEMEM_VIEWER_STATIC_DIR = previousStaticDir;
			rmSync(tmpDir, { recursive: true, force: true });
		}
	});

	describe("GET /api/stats", () => {
		it("returns database stats", async () => {
			const { app, cleanup } = createTestApp();
			try {
				const res = await app.request("/api/stats");
				expect(res.status).toBe(200);
				const body = (await res.json()) as Record<string, unknown>;
				expect(body).toHaveProperty("database");
				expect(typeof body.viewer_pid).toBe("number");
				const db = body.database as Record<string, unknown>;
				expect(db).toHaveProperty("path");
				expect(db).toHaveProperty("sessions");
				expect(db).toHaveProperty("memory_items");
			} finally {
				cleanup();
			}
		});

		it("counts only visible memory scopes in memory stats", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				grantSyncScopeToDevices(store, "authorized-team", [store.deviceId]);
				grantSyncScopeToDevices(store, "unauthorized-team", []);
				const sessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Visible stats memory",
					scopeId: "authorized-team",
				});
				insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Hidden stats memory",
					scopeId: "unauthorized-team",
				});

				const res = await app.request("/api/stats");
				expect(res.status).toBe(200);
				const body = (await res.json()) as { database: Record<string, number> };
				expect(body.database.memory_items).toBe(1);
				expect(body.database.active_memory_items).toBe(1);
			} finally {
				cleanup();
			}
		});

		it("keeps active maintenance jobs in stable started order", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");

				startMaintenanceJob(store.db, {
					kind: "job-a",
					title: "Job A",
					message: "A",
					progressTotal: 10,
				});
				startMaintenanceJob(store.db, {
					kind: "job-b",
					title: "Job B",
					message: "B",
					progressTotal: 10,
				});
				updateMaintenanceJob(store.db, "job-b", {
					message: "B updated",
					progressCurrent: 5,
				});

				const res = await app.request("/api/stats");
				expect(res.status).toBe(200);
				const body = (await res.json()) as Record<string, unknown>;
				const jobs = body.maintenance_jobs as Array<Record<string, unknown>>;
				expect(jobs.map((job) => job.kind)).toEqual(["job-a", "job-b"]);
			} finally {
				cleanup();
			}
		});
	});

	describe("GET /api/runtime", () => {
		it("returns viewer runtime version info", async () => {
			const { app, cleanup } = createTestApp();
			try {
				const res = await app.request("/api/runtime");
				expect(res.status).toBe(200);
				const body = (await res.json()) as Record<string, unknown>;
				expect(body).toEqual({ version: VERSION });
			} finally {
				cleanup();
			}
		});
	});

	describe("GET /api/usage", () => {
		it("returns recent pack rows for the current scope", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				const sessionId = insertTestSession(store.db);
				store.db.prepare("UPDATE sessions SET project = ? WHERE id = ?").run("codemem", sessionId);
				insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Usage-visible memory",
				});
				store.db
					.prepare(
						`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
						 VALUES (?, 'pack', 123, 0, 456, ?, ?)`,
					)
					.run(
						sessionId,
						"2026-03-26T23:30:00Z",
						JSON.stringify({ pack_tokens: 123, exact_duplicates_collapsed: 4 }),
					);

				const res = await app.request("/api/usage?project=codemem");
				expect(res.status).toBe(200);
				const body = (await res.json()) as Record<string, unknown>;
				const recentPacks = body.recent_packs as Array<Record<string, unknown>>;
				expect(recentPacks).toHaveLength(1);
				expect(recentPacks[0]).toMatchObject({
					session_id: sessionId,
					event: "pack",
					tokens_read: 123,
					tokens_saved: 456,
				});
				expect(recentPacks[0]?.metadata_json).toMatchObject({
					exact_duplicates_collapsed: 4,
				});
			} finally {
				cleanup();
			}
		});

		it("removes hidden memory ids from recent pack metadata", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				grantSyncScopeToDevices(store, "authorized-team", [store.deviceId]);
				grantSyncScopeToDevices(store, "unauthorized-team", []);
				const sessionId = insertTestSession(store.db);
				const visibleId = insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Visible pack item",
					scopeId: "authorized-team",
				});
				const hiddenId = insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Hidden pack item",
					scopeId: "unauthorized-team",
				});
				const inactiveId = insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Forgotten pack item",
					scopeId: "authorized-team",
					active: false,
				});
				store.db
					.prepare(
						`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
						 VALUES (?, 'pack', 123, 0, 456, ?, ?)`,
					)
					.run(
						sessionId,
						"2026-03-26T23:30:00Z",
						JSON.stringify({
							pack_item_ids: [visibleId, hiddenId, inactiveId],
							added_ids: [visibleId, hiddenId, inactiveId],
							removed_ids: [hiddenId, inactiveId],
							retained_ids: [String(visibleId), String(hiddenId), String(inactiveId)],
						}),
					);
				const hiddenSessionId = insertTestSession(store.db);
				const hiddenOnlyId = insertTestMemory(store, {
					sessionId: hiddenSessionId,
					kind: "discovery",
					title: "Hidden only pack item",
					scopeId: "unauthorized-team",
				});
				store.db
					.prepare(
						`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
						 VALUES (?, 'pack', 999, 0, 999, ?, ?)`,
					)
					.run(
						hiddenSessionId,
						"2026-03-27T23:30:00Z",
						JSON.stringify({ pack_item_ids: [hiddenOnlyId], project: "secret-project" }),
					);

				const res = await app.request("/api/usage");
				expect(res.status).toBe(200);
				const body = (await res.json()) as {
					recent_packs: Array<{ metadata_json: unknown }>;
					totals: { count: number; tokens_read: number; tokens_saved: number };
				};
				// recent_packs stays scope-filtered: only the visible-session pack
				// survives, with hidden ids stripped from its metadata.
				expect(body.recent_packs).toHaveLength(1);
				// Aggregate totals apply the same visibility predicate before the
				// SQL sum, so the hidden-session pack is excluded from totals too.
				expect(body.totals).toMatchObject({ count: 1, tokens_read: 123, tokens_saved: 456 });
				expect(body.recent_packs[0]?.metadata_json).toMatchObject({
					pack_item_ids: [visibleId],
					added_ids: [visibleId],
					removed_ids: [],
					retained_ids: [visibleId],
				});
			} finally {
				cleanup();
			}
		});

		it("does not expose hidden usage rows that reference visible pack ids", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				grantSyncScopeToDevices(store, "authorized-team", [store.deviceId]);
				grantSyncScopeToDevices(store, "unauthorized-team", []);
				const visibleSessionId = insertTestSession(store.db);
				const visibleId = insertTestMemory(store, {
					sessionId: visibleSessionId,
					kind: "discovery",
					title: "Visible pack item from another session",
					scopeId: "authorized-team",
				});
				const hiddenSessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId: hiddenSessionId,
					kind: "discovery",
					title: "Hidden session item",
					scopeId: "unauthorized-team",
				});
				store.db
					.prepare(
						`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
						 VALUES (?, 'pack', 999, 0, 999, ?, ?)`,
					)
					.run(
						hiddenSessionId,
						"2026-03-29T23:30:00Z",
						JSON.stringify({ pack_item_ids: [visibleId], project: "secret-project" }),
					);

				const res = await app.request("/api/usage");
				expect(res.status).toBe(200);
				const body = (await res.json()) as {
					recent_packs: unknown[];
					totals: { count: number; tokens_read: number; tokens_saved: number };
				};
				// The hidden-session pack is excluded from recent_packs (session
				// not visible) and the aggregate totals apply the same visibility
				// predicate before summing, so it contributes nothing there either.
				expect(body.recent_packs).toHaveLength(0);
				expect(body.totals).toMatchObject({ count: 0, tokens_read: 0, tokens_saved: 0 });
			} finally {
				cleanup();
			}
		});

		it("batches usage memory visibility instead of fetching each pack item", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				const sessionId = insertTestSession(store.db);
				const visibleIds = Array.from({ length: 25 }, (_, idx) =>
					insertTestMemory(store, {
						sessionId,
						kind: "discovery",
						title: `Visible usage item ${idx}`,
					}),
				);
				store.db
					.prepare(
						`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
						 VALUES (?, 'pack', 123, 0, 456, ?, ?)`,
					)
					.run(
						sessionId,
						"2026-03-28T23:30:00Z",
						JSON.stringify({ pack_item_ids: visibleIds, added_ids: visibleIds }),
					);
				const getSpy = vi.spyOn(store, "get");

				const res = await app.request("/api/usage");

				expect(res.status).toBe(200);
				expect(getSpy).not.toHaveBeenCalled();
				const body = (await res.json()) as { recent_packs: Array<{ metadata_json: unknown }> };
				expect(body.recent_packs[0]?.metadata_json).toMatchObject({
					pack_item_ids: visibleIds,
					added_ids: visibleIds,
				});
			} finally {
				cleanup();
			}
		});

		it("serves a cached usage payload within the short TTL window", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				const sessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Cached usage memory",
				});
				const insertPack = (createdAt: string) =>
					store.db
						.prepare(
							`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
							 VALUES (?, 'pack', 100, 0, 200, ?, '{}')`,
						)
						.run(sessionId, createdAt);

				insertPack("2026-03-26T23:30:00Z");
				const first = (await (await app.request("/api/usage")).json()) as {
					totals: { count: number };
				};
				expect(first.totals.count).toBe(1);

				// A second pack inserted immediately should NOT change the cached
				// response while the TTL window is still open.
				insertPack("2026-03-26T23:31:00Z");
				const second = (await (await app.request("/api/usage")).json()) as {
					totals: { count: number };
				};
				expect(second.totals.count).toBe(1);
			} finally {
				cleanup();
			}
		});

		it("busts the usage cache when scope visibility changes within the TTL", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				grantSyncScopeToDevices(store, "authorized-team", [store.deviceId]);
				const sessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Scoped usage memory",
					scopeId: "authorized-team",
				});
				store.db
					.prepare(
						`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
						 VALUES (?, 'pack', 100, 0, 200, ?, '{}')`,
					)
					.run(sessionId, "2026-03-26T23:30:00Z");

				const beforeRevoke = (await (await app.request("/api/usage")).json()) as {
					recent_packs: unknown[];
				};
				expect(beforeRevoke.recent_packs).toHaveLength(1);

				// Revoke the device's membership. Even within the TTL window the
				// next request must recompute and hide the now-invisible scope
				// instead of serving the cached (visible) payload. recent_packs is
				// the scope-sensitive surface here (aggregate totals are now
				// unfiltered, so they would not reflect a visibility change).
				store.db
					.prepare("DELETE FROM scope_memberships WHERE scope_id = ? AND device_id = ?")
					.run("authorized-team", store.deviceId);

				const afterRevoke = (await (await app.request("/api/usage")).json()) as {
					recent_packs: unknown[];
				};
				expect(afterRevoke.recent_packs).toHaveLength(0);
			} finally {
				cleanup();
			}
		});

		it("evicts expired usage-cache entries on sweep", () => {
			const { cache, sweep } = __usageCacheTestHooks;
			cache.clear();
			try {
				const nowMs = 1_000_000;
				// One already-expired entry and one still-live entry.
				cache.set("expired-key", { payload: {}, expiresAtMs: nowMs - 1 });
				cache.set("live-key", { payload: {}, expiresAtMs: nowMs + 10_000 });
				sweep(cache, nowMs);
				expect(cache.has("expired-key")).toBe(false);
				expect(cache.has("live-key")).toBe(true);
				expect(cache.size).toBe(1);
			} finally {
				cache.clear();
			}
		});

		it("caps the usage cache under a flood of distinct /api/usage requests", async () => {
			const { app, cleanup } = createTestApp();
			const { cache, maxEntries } = __usageCacheTestHooks;
			cache.clear();
			try {
				await app.request("/api/stats");
				// Each distinct ?project= yields a distinct cache key (see
				// usageCacheKey), mirroring the per-request-unique key growth the
				// sweep defends against. Driving the real endpoint guards the
				// handler's use of the sweep, not just the helper in isolation — if
				// the sweep call is ever removed from the handler, this fails.
				for (let i = 0; i < maxEntries + 50; i += 1) {
					await app.request(`/api/usage?project=flood-${i}`);
				}
				expect(cache.size).toBeGreaterThan(1);
				expect(cache.size).toBeLessThanOrEqual(maxEntries);
			} finally {
				cache.clear();
				cleanup();
			}
		});

		it("aggregates token/event totals in SQL with hand-summed global and project values", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				const codememSession = insertTestSession(store.db);
				store.db
					.prepare("UPDATE sessions SET project = ? WHERE id = ?")
					.run("codemem", codememSession);
				const otherSession = insertTestSession(store.db);
				store.db.prepare("UPDATE sessions SET project = ? WHERE id = ?").run("other", otherSession);
				// The usage aggregates only count rows whose session is visible, so
				// give each session one eligible memory.
				insertTestMemory(store, {
					sessionId: codememSession,
					kind: "discovery",
					title: "Codemem visible memory",
				});
				insertTestMemory(store, {
					sessionId: otherSession,
					kind: "discovery",
					title: "Other visible memory",
				});

				const insertUsage = (
					sessionId: number,
					event: string,
					read: number,
					written: number,
					saved: number | null,
					createdAt: string,
				) =>
					store.db
						.prepare(
							`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
							 VALUES (?, ?, ?, ?, ?, ?, '{}')`,
						)
						.run(sessionId, event, read, written, saved, createdAt);

				// codemem project: two packs + one search.
				insertUsage(codememSession, "pack", 100, 10, 5, "2026-03-26T23:30:00Z");
				insertUsage(codememSession, "pack", 200, 20, null, "2026-03-26T23:31:00Z");
				insertUsage(codememSession, "search", 30, 3, 7, "2026-03-26T23:32:00Z");
				// other project: one pack.
				insertUsage(otherSession, "pack", 1000, 100, 50, "2026-03-26T23:33:00Z");

				const res = await app.request("/api/usage?project=codemem");
				expect(res.status).toBe(200);
				const body = (await res.json()) as {
					events_global: Array<{
						event: string;
						total_tokens_read: number;
						total_tokens_written: number;
						total_tokens_saved: number;
						count: number;
					}>;
					totals_global: {
						tokens_read: number;
						tokens_written: number;
						tokens_saved: number;
						count: number;
					};
					events_filtered: Array<{
						event: string;
						total_tokens_read: number;
						total_tokens_written: number;
						total_tokens_saved: number;
						count: number;
					}> | null;
					totals_filtered: {
						tokens_read: number;
						tokens_written: number;
						tokens_saved: number;
						count: number;
					} | null;
					totals: {
						tokens_read: number;
						tokens_written: number;
						tokens_saved: number;
						count: number;
					};
				};

				// Global aggregate = all four rows, NULL tokens_saved coalesced to 0.
				expect(body.totals_global).toEqual({
					tokens_read: 1330,
					tokens_written: 133,
					tokens_saved: 62,
					count: 4,
				});
				// events_global is sorted by event name ASC (pack before search).
				expect(body.events_global).toEqual([
					{
						event: "pack",
						total_tokens_read: 1300,
						total_tokens_written: 130,
						total_tokens_saved: 55,
						count: 3,
					},
					{
						event: "search",
						total_tokens_read: 30,
						total_tokens_written: 3,
						total_tokens_saved: 7,
						count: 1,
					},
				]);

				// Project-filtered aggregate = only the codemem rows.
				expect(body.totals_filtered).toEqual({
					tokens_read: 330,
					tokens_written: 33,
					tokens_saved: 12,
					count: 3,
				});
				expect(body.events_filtered).toEqual([
					{
						event: "pack",
						total_tokens_read: 300,
						total_tokens_written: 30,
						total_tokens_saved: 5,
						count: 2,
					},
					{
						event: "search",
						total_tokens_read: 30,
						total_tokens_written: 3,
						total_tokens_saved: 7,
						count: 1,
					},
				]);

				// When a project filter is present, `totals` mirrors the filtered values.
				expect(body.totals).toEqual(body.totals_filtered);
			} finally {
				cleanup();
			}
		});

		it("orders recent_packs by created_at DESC and caps the surfaced window", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				const sessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Recent-pack visible memory",
				});
				const insertPack = (createdAt: string, tokensRead: number) =>
					store.db
						.prepare(
							`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
							 VALUES (?, 'pack', ?, 0, 0, ?, '{}')`,
						)
						.run(sessionId, tokensRead, createdAt);

				// Insert 15 packs in ascending time order; the route should return
				// the newest first and never surface more than 10.
				for (let i = 0; i < 15; i += 1) {
					const minute = String(i).padStart(2, "0");
					insertPack(`2026-03-26T23:${minute}:00Z`, i);
				}

				const res = await app.request("/api/usage");
				expect(res.status).toBe(200);
				const body = (await res.json()) as {
					recent_packs: Array<{ created_at: string }>;
				};
				expect(body.recent_packs).toHaveLength(10);
				const timestamps = body.recent_packs.map((row) => row.created_at);
				const sortedDesc = [...timestamps].sort((a, b) => b.localeCompare(a));
				expect(timestamps).toEqual(sortedDesc);
				// Newest seeded pack (minute 14) is first; oldest surfaced is minute 05.
				expect(timestamps[0]).toBe("2026-03-26T23:14:00Z");
				expect(timestamps.at(-1)).toBe("2026-03-26T23:05:00Z");
			} finally {
				cleanup();
			}
		});

		it("keeps a visible pack inside the bounded recent-pack window despite newer non-visible packs", async () => {
			// The visibility predicate is applied in SQL before the bounded
			// recent_packs window, so newer non-visible packs cannot starve a
			// visible pack out of the window, and the aggregate counts only
			// visible events.
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				const sessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId,
					kind: "discovery",
					title: "Starvation-test visible memory",
				});
				const insertPack = (sessionRef: number | null, createdAt: string) =>
					store.db
						.prepare(
							`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
							 VALUES (?, 'pack', 1, 0, 0, ?, '{}')`,
						)
						.run(sessionRef, createdAt);

				// One visible pack at the OLDEST timestamp (session is visible)...
				insertPack(sessionId, "2026-03-26T00:00:00Z");
				// ...buried under 200 NEWER non-visible packs (NULL session => a row
				// with no pack_item_ids and a null session is never visible). The
				// newest-200 window is entirely non-visible, so the lone visible pack
				// sits at position 201 and is never considered.
				for (let i = 0; i < 200; i += 1) {
					const minute = String(Math.floor(i / 60)).padStart(2, "0");
					const second = String(i % 60).padStart(2, "0");
					insertPack(null, `2026-04-01T00:${minute}:${second}Z`);
				}

				const res = await app.request("/api/usage");
				expect(res.status).toBe(200);
				const body = (await res.json()) as {
					recent_packs: unknown[];
					totals_global: { count: number };
				};
				// The visible pack survives the window despite 200 newer non-visible packs...
				expect(body.recent_packs).toHaveLength(1);
				// ...and the aggregate counts only the visible pack event.
				expect(body.totals_global.count).toBe(1);
			} finally {
				cleanup();
			}
		});
	});

	describe("GET /api/sessions", () => {
		it("returns sessions list", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				// Force store creation
				const _warmup = await app.request("/api/stats");
				const store = getStore();
				if (store) {
					const sessionId = insertTestSession(store.db);
					insertTestMemory(store, {
						sessionId,
						kind: "discovery",
						title: "Visible session memory",
					});
				}
				const res = await app.request("/api/sessions");
				expect(res.status).toBe(200);
				const body = (await res.json()) as Record<string, unknown>;
				expect(body).toHaveProperty("items");
				const items = body.items as Record<string, unknown>[];
				expect(items.length).toBeGreaterThanOrEqual(1);
			} finally {
				cleanup();
			}
		});
	});

	describe("GET /api/projects", () => {
		it("returns empty projects for fresh DB", async () => {
			const { app, cleanup } = createTestApp();
			try {
				const res = await app.request("/api/projects");
				expect(res.status).toBe(200);
				const body = (await res.json()) as Record<string, unknown>;
				expect(body).toHaveProperty("projects");
			} finally {
				cleanup();
			}
		});

		it("only lists projects backed by visible memory scopes", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				grantSyncScopeToDevices(store, "authorized-team", [store.deviceId]);
				grantSyncScopeToDevices(store, "unauthorized-team", []);

				const visibleSessionId = insertTestSession(store.db);
				store.db
					.prepare("UPDATE sessions SET project = ? WHERE id = ?")
					.run("visible-project", visibleSessionId);
				insertTestMemory(store, {
					sessionId: visibleSessionId,
					kind: "discovery",
					title: "Visible scoped memory",
					scopeId: "authorized-team",
				});

				const hiddenSessionId = insertTestSession(store.db);
				store.db
					.prepare("UPDATE sessions SET project = ? WHERE id = ?")
					.run("secret-project", hiddenSessionId);
				insertTestMemory(store, {
					sessionId: hiddenSessionId,
					kind: "discovery",
					title: "Hidden scoped memory",
					scopeId: "unauthorized-team",
				});

				const projectsRes = await app.request("/api/projects");
				expect(projectsRes.status).toBe(200);
				const projectsBody = (await projectsRes.json()) as { projects: string[] };
				expect(projectsBody.projects).toEqual(["visible-project"]);

				const sessionsRes = await app.request("/api/sessions");
				expect(sessionsRes.status).toBe(200);
				const sessionsBody = (await sessionsRes.json()) as {
					items: Array<{ id: number; project: string }>;
				};
				expect(sessionsBody.items.map((item) => item.id)).toEqual([visibleSessionId]);
			} finally {
				cleanup();
			}
		});
	});

	describe("GET /api/artifacts", () => {
		it("requires a visible memory in the session before returning local artifacts", async () => {
			const { app, getStore, cleanup } = createTestApp();
			try {
				await app.request("/api/stats");
				const store = getStore();
				if (!store) throw new Error("store not initialized");
				grantSyncScopeToDevices(store, "authorized-team", [store.deviceId]);
				grantSyncScopeToDevices(store, "unauthorized-team", []);

				const visibleSessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId: visibleSessionId,
					kind: "discovery",
					title: "Visible artifact session memory",
					scopeId: "authorized-team",
				});
				store.db
					.prepare(
						`INSERT INTO artifacts(session_id, kind, path, content_text, content_hash, created_at, metadata_json, sensitivity)
						 VALUES (?, 'note', 'visible.txt', 'visible artifact', 'visible-hash', ?, '{}', 'eligible')`,
					)
					.run(visibleSessionId, "2026-01-01T00:00:00Z");

				const hiddenSessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId: hiddenSessionId,
					kind: "discovery",
					title: "Hidden artifact session memory",
					scopeId: "unauthorized-team",
				});
				store.db
					.prepare(
						`INSERT INTO artifacts(session_id, kind, path, content_text, content_hash, created_at, metadata_json, sensitivity)
						 VALUES (?, 'note', 'hidden.txt', 'hidden artifact', 'hidden-hash', ?, '{}', 'eligible')`,
					)
					.run(hiddenSessionId, "2026-01-01T00:00:00Z");

				const mixedSessionId = insertTestSession(store.db);
				insertTestMemory(store, {
					sessionId: mixedSessionId,
					kind: "discovery",
					title: "Mixed visible artifact memory",
					scopeId: "authorized-team",
				});
				insertTestMemory(store, {
					sessionId: mixedSessionId,
					kind: "discovery",
					title: "Mixed hidden artifact memory",
					scopeId: "unauthorized-team",
				});
				store.db
					.prepare(
						`INSERT INTO artifacts(session_id, kind, path, content_text, content_hash, created_at, metadata_json, sensitivity)
						 VALUES (?, 'note', 'mixed.txt', 'mixed artifact', 'mixed-hash', ?, '{}', 'eligible')`,
					)
					.run(mixedSessionId, "2026-01-01T00:00:00Z");

				const visibleRes = await app.request(`/api/artifacts?session_id=${visibleSessionId}`);
				expect(visibleRes.status).toBe(200);
				const visibleBody = (await visibleRes.json()) as { items: Array<{ path: string }> };
				expect(visibleBody.items.map((item) => item.path)).toEqual(["visible.txt"]);

				const hiddenRes = await app.request(`/api/artifacts?session_id=${hiddenSessionId}`);
				expect(hiddenRes.status).toBe(404);
				expect(await hiddenRes.json()).toEqual({ error: "session not found" });

				const mixedRes = await app.request(`/api/artifacts?session_id=${mixedSessionId}`);
				expect(mixedRes.status).toBe(404);
				expect(await mixedRes.json()).toEqual({ error: "session not found" });
			} finally {
				cleanup();
			}
		});
	});

	describe("viewer HTML", () => {
		it("returns HTML at root with viewer page", async () => {
			const { app, cleanup } = createTestApp();
			try {
				const res = await app.request("/");
				expect(res.status).toBe(200);
				const html = await res.text();
				expect(html).toContain("<title>codemem viewer</title>");
				expect(html).toContain("<!doctype html>");
			} finally {
				cleanup();
			}
		});
	});
});
