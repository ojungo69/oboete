import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
	compileCapabilityManifest,
	compileDefaultCapabilityManifest,
} from "./capability-manifest.js";
import { compileProviderDestinationBoundary } from "./destination-boundary.js";
import { buildFilterClauses, buildFilterClausesWithContext } from "./filters.js";
import { type MemoryStore, ProcessingResumeError } from "./store.js";
import { insertTestSession, openTestMemoryStore } from "./test-utils.js";
import * as vectors from "./vectors.js";

// ---------------------------------------------------------------------------
// Helper: create a MemoryStore backed by a migrated temp DB.
// ---------------------------------------------------------------------------

describe("MemoryStore", () => {
	let tmpDir: string;
	let dbPath: string;
	let store: MemoryStore;
	let prevCodememConfig: string | undefined;
	let prevActorId: string | undefined;
	let prevActorDisplayName: string | undefined;
	let prevCrossSessionDedupWindowMs: string | undefined;
	let prevCodememDebug: string | undefined;

	beforeEach(() => {
		prevCodememConfig = process.env.CODEMEM_CONFIG;
		prevActorId = process.env.CODEMEM_ACTOR_ID;
		prevActorDisplayName = process.env.CODEMEM_ACTOR_DISPLAY_NAME;
		prevCrossSessionDedupWindowMs = process.env.CODEMEM_MEMORY_CROSS_SESSION_DEDUP_WINDOW_MS;
		prevCodememDebug = process.env.CODEMEM_DEBUG;
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-store-test-"));
		process.env.CODEMEM_CONFIG = join(tmpDir, "config.json");
		delete process.env.CODEMEM_ACTOR_ID;
		delete process.env.CODEMEM_ACTOR_DISPLAY_NAME;
		delete process.env.CODEMEM_MEMORY_CROSS_SESSION_DEDUP_WINDOW_MS;
		delete process.env.CODEMEM_DEBUG;
		dbPath = join(tmpDir, "test.sqlite");
		store = openTestMemoryStore(dbPath);
	});

	afterEach(() => {
		store?.close();
		if (prevCodememConfig === undefined) delete process.env.CODEMEM_CONFIG;
		else process.env.CODEMEM_CONFIG = prevCodememConfig;
		if (prevActorId === undefined) delete process.env.CODEMEM_ACTOR_ID;
		else process.env.CODEMEM_ACTOR_ID = prevActorId;
		if (prevActorDisplayName === undefined) delete process.env.CODEMEM_ACTOR_DISPLAY_NAME;
		else process.env.CODEMEM_ACTOR_DISPLAY_NAME = prevActorDisplayName;
		if (prevCrossSessionDedupWindowMs === undefined) {
			delete process.env.CODEMEM_MEMORY_CROSS_SESSION_DEDUP_WINDOW_MS;
		} else {
			process.env.CODEMEM_MEMORY_CROSS_SESSION_DEDUP_WINDOW_MS = prevCrossSessionDedupWindowMs;
		}
		if (prevCodememDebug === undefined) delete process.env.CODEMEM_DEBUG;
		else process.env.CODEMEM_DEBUG = prevCodememDebug;
		rmSync(tmpDir, { recursive: true, force: true });
	});

	function insertCoordinatorScope(scopeId: string): void {
		const now = new Date().toISOString();
		store.db
			.prepare(
				`INSERT OR REPLACE INTO replication_scopes(
					scope_id, label, kind, authority_type, coordinator_id, group_id,
					membership_epoch, status, created_at, updated_at
				 ) VALUES (?, ?, 'team', 'coordinator', 'coord-test', 'group-test', 0, 'active', ?, ?)`,
			)
			.run(scopeId, scopeId, now, now);
	}

	function grantScopeToLocalDevice(scopeId: string): void {
		insertCoordinatorScope(scopeId);
		store.db
			.prepare(
				`INSERT OR REPLACE INTO scope_memberships(
					scope_id, device_id, role, status, membership_epoch,
					coordinator_id, group_id, updated_at
				 ) VALUES (?, ?, 'member', 'active', 0, 'coord-test', 'group-test', ?)`,
			)
			.run(scopeId, store.deviceId, new Date().toISOString());
	}

	function insertScopedMemory(scopeId: string, title: string): number {
		const sessionId = insertTestSession(store.db);
		const now = new Date().toISOString();
		const info = store.db
			.prepare(
				`INSERT INTO memory_items(
					session_id, kind, title, body_text, confidence, tags_text, active,
					created_at, updated_at, metadata_json, rev, scope_id
				 ) VALUES (?, 'discovery', ?, 'scope body', 0.5, '', 1, ?, ?, '{}', 1, ?)`,
			)
			.run(sessionId, title, now, now, scopeId);
		return Number(info.lastInsertRowid);
	}

	// -- get ----------------------------------------------------------------

	describe("get", () => {
		it("returns null for non-existent memory", () => {
			expect(store.get(9999)).toBeNull();
		});

		it("returns a memory item with parsed metadata", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Test title", "Test body");

			const result = store.get(memId);
			expect(result).not.toBeNull();
			expect(result?.id).toBe(memId);
			expect(result?.kind).toBe("discovery");
			expect(result?.title).toBe("Test title");
			expect(result?.body_text).toBe("Test body");
			// metadata_json should be parsed into an object
			expect(typeof result?.metadata_json).toBe("object");
		});

		it("hides direct ID reads outside locally authorized scopes", () => {
			grantScopeToLocalDevice("authorized-team");
			insertCoordinatorScope("unauthorized-team");
			const visibleId = insertScopedMemory("authorized-team", "Authorized memory");
			const hiddenId = insertScopedMemory("unauthorized-team", "Unauthorized memory");

			expect(store.get(visibleId)?.title).toBe("Authorized memory");
			expect(store.get(hiddenId)).toBeNull();
		});
	});

	// -- remember -----------------------------------------------------------

	describe("remember", () => {
		it("defaults deviceId to stable 'local' when sync_device is empty", () => {
			expect(store.deviceId).toBe("local");
			expect(store.actorId).toBe("local:local");
		});

		it("refreshes a proven persisted local identity without restart", () => {
			const now = "2026-07-23T12:00:00.000Z";
			store.db
				.prepare(
					`INSERT INTO actors(actor_id, display_name, is_local, status, created_at, updated_at)
					 VALUES ('identity-reviewed', 'Reviewed Owner', 1, 'active', ?, ?)`,
				)
				.run(now, now);

			expect(store.refreshPersistedLocalIdentity("identity-reviewed")).toBe(true);
			expect(store.actorId).toBe("identity-reviewed");
			expect(store.actorDisplayName).toBe("Reviewed Owner");

			store.adoptEnsuredDeviceIdentity("device-after-refresh");
			expect(store.deviceId).toBe("device-after-refresh");
			expect(store.actorId).toBe("identity-reviewed");
			expect(store.actorDisplayName).toBe("Reviewed Owner");
		});

		it.each([
			["missing", null],
			["remote", { isLocal: 0, status: "active", mergedInto: null }],
			["inactive", { isLocal: 1, status: "inactive", mergedInto: null }],
			["merged", { isLocal: 1, status: "active", mergedInto: "identity-canonical" }],
		] as const)("does not refresh an unproven persisted identity: %s", (_label, candidate) => {
			const originalActorId = store.actorId;
			const originalDisplayName = store.actorDisplayName;
			if (candidate) {
				const now = "2026-07-23T12:00:00.000Z";
				store.db
					.prepare(
						`INSERT INTO actors(
						 actor_id, display_name, is_local, status, merged_into_actor_id, created_at, updated_at
						 ) VALUES ('identity-unproven', 'Unproven', ?, ?, ?, ?, ?)`,
					)
					.run(candidate.isLocal, candidate.status, candidate.mergedInto, now, now);
			}

			expect(store.refreshPersistedLocalIdentity("identity-unproven")).toBe(false);
			expect(store.actorId).toBe(originalActorId);
			expect(store.actorDisplayName).toBe(originalDisplayName);
		});

		it("adopts an ensured device identity and retires the fallback local actor", () => {
			const now = "2026-07-23T12:00:00.000Z";
			store.db
				.prepare(
					`INSERT INTO actors(actor_id, display_name, is_local, status, created_at, updated_at)
					 VALUES ('local:local', 'Dogfood Owner', 1, 'active', ?, ?)`,
				)
				.run(now, now);

			store.adoptEnsuredDeviceIdentity("device-stable-owner");

			expect(store.deviceId).toBe("device-stable-owner");
			expect(store.actorId).toBe("local:device-stable-owner");
			expect(store.actorDisplayName).toBe("Dogfood Owner");
			expect(
				store.db
					.prepare(
						"SELECT actor_id, display_name, is_local, status, merged_into_actor_id FROM actors ORDER BY actor_id",
					)
					.all(),
			).toEqual([
				{
					actor_id: "local:device-stable-owner",
					display_name: "Dogfood Owner",
					is_local: 1,
					status: "active",
					merged_into_actor_id: null,
				},
				{
					actor_id: "local:local",
					display_name: "Dogfood Owner",
					is_local: 0,
					status: "merged",
					merged_into_actor_id: "local:device-stable-owner",
				},
			]);
		});

		it("migrates fallback-authored memory ownership without rewriting historical device clocks", () => {
			const sessionId = insertTestSession(store.db);
			const memoryId = store.remember(sessionId, "discovery", "Fallback memory", "Body", 0.5, [], {
				visibility: "private",
			});

			store.adoptEnsuredDeviceIdentity("device-stable-memory");

			const memory = store.db
				.prepare(
					`SELECT actor_id, actor_display_name, origin_device_id, workspace_id, metadata_json
					 FROM memory_items WHERE id = ?`,
				)
				.get(memoryId) as {
				actor_id: string;
				actor_display_name: string;
				origin_device_id: string;
				workspace_id: string;
				metadata_json: string;
			};
			expect(memory).toMatchObject({
				actor_id: "local:device-stable-memory",
				origin_device_id: "local",
				workspace_id: "personal:local:device-stable-memory",
			});
			expect(JSON.parse(memory.metadata_json)).toMatchObject({ clock_device_id: "local" });
			expect(store.memoryOwnedBySelf(memory)).toBe(true);
			expect(store.recent(10, { ownership_scope: "mine" }).map((item) => item.id)).toContain(
				memoryId,
			);
		});

		it("keeps configured actor identity while adopting the ensured device", () => {
			store.close();
			writeFileSync(
				process.env.CODEMEM_CONFIG as string,
				JSON.stringify({ actor_id: "actor:configured", actor_display_name: "Configured User" }),
			);
			store = openTestMemoryStore(dbPath);

			store.adoptEnsuredDeviceIdentity("device-for-configured-actor");

			expect(store.deviceId).toBe("device-for-configured-actor");
			expect(store.actorId).toBe("actor:configured");
			expect(store.actorDisplayName).toBe("Configured User");
		});

		it("keeps a configured actor canonical while retiring an active stale fallback actor", () => {
			store.close();
			writeFileSync(
				process.env.CODEMEM_CONFIG as string,
				JSON.stringify({ actor_id: "actor:configured", actor_display_name: "Configured User" }),
			);
			store = openTestMemoryStore(dbPath);
			const now = "2026-07-23T12:00:00.000Z";
			store.db
				.prepare(
					`INSERT INTO actors(actor_id, display_name, is_local, status, created_at, updated_at)
					 VALUES ('local:local', 'local:local', 1, 'active', ?, ?)`,
				)
				.run(now, now);

			store.adoptEnsuredDeviceIdentity("device-for-configured-actor");

			expect(store.deviceId).toBe("device-for-configured-actor");
			expect(store.actorId).toBe("actor:configured");
			expect(store.actorDisplayName).toBe("Configured User");
			expect(
				store.db
					.prepare(
						"SELECT actor_id, display_name FROM actors WHERE is_local = 1 AND status = 'active'",
					)
					.all(),
			).toEqual([{ actor_id: "actor:configured", display_name: "Configured User" }]);
			expect(
				store.db
					.prepare(
						"SELECT is_local, status, merged_into_actor_id FROM actors WHERE actor_id = 'local:local'",
					)
					.get(),
			).toEqual({
				is_local: 0,
				status: "merged",
				merged_into_actor_id: "actor:configured",
			});
		});

		it("does not advance in-memory identity when fallback persistence fails", () => {
			const sessionId = insertTestSession(store.db);
			const memoryId = store.remember(sessionId, "discovery", "Rollback memory", "Body", 0.5, [], {
				visibility: "private",
			});
			const now = "2026-07-23T12:00:00.000Z";
			store.db
				.prepare(
					`INSERT INTO actors(actor_id, display_name, is_local, status, created_at, updated_at)
					 VALUES ('local:local', 'Fallback User', 1, 'active', ?, ?)`,
				)
				.run(now, now);
			store.db.exec(`CREATE TRIGGER reject_fallback_retirement BEFORE UPDATE ON actors
				WHEN OLD.actor_id = 'local:local' BEGIN SELECT RAISE(ABORT, 'retirement failed'); END`);

			expect(() => store.adoptEnsuredDeviceIdentity("device-transaction-failure")).toThrow(
				"retirement failed",
			);
			expect(store.deviceId).toBe("local");
			expect(store.actorId).toBe("local:local");
			expect(store.db.prepare("SELECT actor_id FROM actors ORDER BY actor_id").all()).toEqual([
				{ actor_id: "local:local" },
			]);
			const memory = store.db
				.prepare(
					"SELECT actor_id, workspace_id, origin_device_id, metadata_json FROM memory_items WHERE id = ?",
				)
				.get(memoryId) as {
				actor_id: string;
				workspace_id: string;
				origin_device_id: string;
				metadata_json: string;
			};
			expect(memory).toMatchObject({
				actor_id: "local:local",
				workspace_id: "personal:local:local",
				origin_device_id: "local",
			});
			expect(JSON.parse(memory.metadata_json)).toMatchObject({ clock_device_id: "local" });
		});

		it("does not claim fallback actor rows authored by a foreign device", () => {
			const sessionId = insertTestSession(store.db);
			const memoryId = store.remember(sessionId, "discovery", "Foreign fallback", "Body", 0.5, [], {
				actor_id: "local:local",
				origin_device_id: "foreign-device",
				visibility: "private",
			});

			store.adoptEnsuredDeviceIdentity("device-stable-local");

			const memory = store.db
				.prepare("SELECT actor_id, workspace_id, origin_device_id FROM memory_items WHERE id = ?")
				.get(memoryId) as { actor_id: string; workspace_id: string; origin_device_id: string };
			expect(memory).toEqual({
				actor_id: "local:local",
				workspace_id: "personal:local:local",
				origin_device_id: "foreign-device",
			});
			expect(store.memoryOwnedBySelf(memory)).toBe(false);
			expect(store.recent(10, { ownership_scope: "mine" }).map((item) => item.id)).not.toContain(
				memoryId,
			);
			expect(store.recent(10, { ownership_scope: "theirs" }).map((item) => item.id)).toContain(
				memoryId,
			);
		});

		it("loads actor identity defaults from codemem config file", () => {
			store.close();
			writeFileSync(
				process.env.CODEMEM_CONFIG as string,
				JSON.stringify({ actor_id: "actor:config", actor_display_name: "Config User" }),
			);
			store = openTestMemoryStore(dbPath);

			expect(store.actorId).toBe("actor:config");
			expect(store.actorDisplayName).toBe("Config User");
		});

		it("lets env overrides win over config-backed actor identity", () => {
			store.close();
			writeFileSync(
				process.env.CODEMEM_CONFIG as string,
				JSON.stringify({ actor_id: "actor:config", actor_display_name: "Config User" }),
			);
			process.env.CODEMEM_ACTOR_ID = "actor:env";
			process.env.CODEMEM_ACTOR_DISPLAY_NAME = "Env User";
			store = openTestMemoryStore(dbPath);

			expect(store.actorId).toBe("actor:env");
			expect(store.actorDisplayName).toBe("Env User");
		});

		it("inserts a memory item and returns the ID", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "feature", "My Feature", "Feature body", 0.8);

			expect(memId).toBeGreaterThan(0);

			const row = store.get(memId);
			expect(row).not.toBeNull();
			expect(row?.kind).toBe("feature");
			expect(row?.title).toBe("My Feature");
			expect(row?.body_text).toBe("Feature body");
			expect(row?.confidence).toBe(0.8);
			expect(row?.active).toBe(1);
			expect(row?.rev).toBe(1);
			expect(row?.deleted_at).toBeNull();
		});

		it("redacts secrets in title, body, and metadata before persisting", () => {
			const sessionId = insertTestSession(store.db);
			const pat = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
			const awsId = "AKIAIOSFODNN7EXAMPLE";
			const memId = store.remember(
				sessionId,
				"discovery",
				`Found token ${pat} in config`,
				`Body has ${awsId} embedded`,
				0.5,
				undefined,
				{ password: "supersecretvalue123", note: "harmless" },
			);

			const row = store.get(memId);
			expect(row?.title).toContain("[REDACTED:github_pat_classic]");
			expect(row?.title).not.toContain(pat);
			expect(row?.body_text).toContain("[REDACTED:aws_access_key_id]");
			expect(row?.body_text).not.toContain(awsId);
			const meta = row?.metadata_json as Record<string, unknown>;
			expect(meta.password).toBe("[REDACTED:context_secret]");
			expect(meta.note).toBe("harmless");
		});

		it("redacts secrets in tags before persisting", () => {
			const sessionId = insertTestSession(store.db);
			const pat = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
			const memId = store.remember(sessionId, "discovery", "Title", "Body", 0.5, ["safe-tag", pat]);
			const row = store.get(memId);
			expect(row?.tags_text).toContain("[REDACTED:github_pat_classic]");
			expect(row?.tags_text).toContain("safe-tag");
			expect(row?.tags_text).not.toContain(pat);
		});

		it("applies workspace-config secret_scanner rules to local writes", () => {
			store.close();
			writeFileSync(
				process.env.CODEMEM_CONFIG as string,
				JSON.stringify({
					secret_scanner: {
						rules: [{ kind: "internal_acme_token", pattern: "\\bACME-[A-Z0-9]{10}\\b" }],
						allowlist: ["AKIAFAKEFIXTURE0001"],
					},
				}),
			);
			store = openTestMemoryStore(dbPath);
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(
				sessionId,
				"discovery",
				"workspace title with ACME-AB12CD34EF token",
				"Body has AKIAFAKEFIXTURE0001 fixture and AKIAIOSFODNN7EXAMPLE real",
			);
			const row = store.get(memId);
			// Workspace rule fires
			expect(row?.title).toContain("[REDACTED:internal_acme_token]");
			expect(row?.title).not.toContain("ACME-AB12CD34EF");
			// Allowlist entry passes through
			expect(row?.body_text).toContain("AKIAFAKEFIXTURE0001");
			// Default rules still active for everything else
			expect(row?.body_text).toContain("[REDACTED:aws_access_key_id]");
			expect(row?.body_text).not.toContain("AKIAIOSFODNN7EXAMPLE");
		});

		it("persists metadata only when workspace scanner config is invalid", () => {
			const invalidConfigs = [
				JSON.stringify({
					secret_scanner: {
						rules: [{ kind: "invalid", pattern: "SECRET-[A-Z]+", redactGroup: 1 }],
					},
				}),
				JSON.stringify({
					secret_scanner: {
						rules: [
							{
								kind: ["ghp_", "abcdefghijklmnopqrstuvwxyz0123456789"].join(""),
								pattern: "INTERNAL-SECRET",
							},
						],
					},
				}),
				"{ invalid JSON",
			];
			for (const invalidConfig of invalidConfigs) {
				store.close();
				writeFileSync(process.env.CODEMEM_CONFIG as string, invalidConfig);
				store = openTestMemoryStore(dbPath);
				const sessionId = insertTestSession(store.db);
				const memId = store.remember(sessionId, "discovery", "Private title", "Private body");
				const row = store.get(memId);
				expect(row?.title).toBe("");
				expect(row?.body_text).toBe("");
				expect(row?.metadata_json).toMatchObject({ redaction_degraded: true });
			}
		});

		it("stamps local-default scope on new memory by default", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "feature", "Scoped default", "Feature body");

			const memory = store.db
				.prepare("SELECT import_key, scope_id FROM memory_items WHERE id = ?")
				.get(memId) as { import_key: string; scope_id: string | null };
			expect(memory.scope_id).toBe("local-default");
		});

		it("stamps mapped scope on new memory", () => {
			const sessionId = insertTestSession(store.db);
			store.db
				.prepare("UPDATE sessions SET cwd = ?, project = ? WHERE id = ?")
				.run("/work/acme/service", "service", sessionId);
			store.db
				.prepare(
					`INSERT INTO project_scope_mappings(
						workspace_identity, project_pattern, scope_id, priority, source, created_at, updated_at
					 ) VALUES (?, ?, ?, ?, ?, ?, ?)`,
				)
				.run(
					"/work/acme/service",
					"/work/acme/*",
					"acme-work",
					10,
					"user",
					"2026-05-01T00:00:00Z",
					"2026-05-01T00:00:00Z",
				);

			const memId = store.remember(sessionId, "feature", "Scoped mapped", "Feature body");
			const memory = store.db
				.prepare("SELECT import_key, scope_id FROM memory_items WHERE id = ?")
				.get(memId) as { import_key: string; scope_id: string | null };
			expect(memory.scope_id).toBe("acme-work");
		});

		it("generates an import_key when not provided", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			const row = store.get(memId);
			expect(row?.import_key).toBeTruthy();
			expect(typeof row?.import_key).toBe("string");
		});

		it("preserves provided import_key in metadata", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body", 0.5, undefined, {
				import_key: "custom-key-123",
			});

			const row = store.get(memId);
			expect(row?.import_key).toBe("custom-key-123");
		});

		it("validates and normalizes memory kind", () => {
			const sessionId = insertTestSession(store.db);
			// Accepts valid kind
			const memId = store.remember(sessionId, "  Discovery  ", "Title", "Body");
			const row = store.get(memId);
			expect(row?.kind).toBe("discovery");
		});

		it("rejects invalid memory kind", () => {
			const sessionId = insertTestSession(store.db);
			expect(() => store.remember(sessionId, "yolo", "Title", "Body")).toThrow(
				/Invalid memory kind/,
			);
		});

		it("succeeds with empty title", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "", "Body with empty title");
			expect(memId).toBeGreaterThan(0);

			const row = store.get(memId);
			expect(row).not.toBeNull();
			expect(row?.title).toBe("");
			expect(row?.body_text).toBe("Body with empty title");
		});

		it("rejects invalid kind with descriptive error including valid kinds", () => {
			const sessionId = insertTestSession(store.db);
			try {
				store.remember(sessionId, "made_up_kind", "Title", "Body");
				expect.unreachable("should have thrown");
			} catch (e) {
				const msg = (e as Error).message;
				expect(msg).toMatch(/Invalid memory kind/);
				expect(msg).toContain("made_up_kind");
			}
		});

		it("sets clock_device_id in metadata", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");
			const row = store.get(memId);
			expect(row?.metadata_json.clock_device_id).toBe(store.deviceId);
		});

		it("sets origin_device_id", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");
			const row = store.get(memId);
			expect(row?.origin_device_id).toBe(store.deviceId);
		});

		it("sorts and deduplicates tags", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "T", "B", 0.5, [
				"beta",
				"alpha",
				"beta",
			]);

			const row = store.get(memId);
			expect(row?.tags_text).toBe("alpha beta");
		});

		it("kicks off vector storage after remembering a memory", () => {
			const storeVectorsSpy = vi.spyOn(vectors, "storeVectors").mockResolvedValue();
			try {
				const sessionId = insertTestSession(store.db);
				const memId = store.remember(sessionId, "feature", "Vector title", "Vector body");

				expect(storeVectorsSpy).toHaveBeenCalledTimes(1);
				expect(storeVectorsSpy).toHaveBeenCalledWith(
					store.db,
					memId,
					"Vector title",
					"Vector body",
				);
			} finally {
				storeVectorsSpy.mockRestore();
			}
		});

		it("does not fail remember when vector storage fails", () => {
			const storeVectorsSpy = vi
				.spyOn(vectors, "storeVectors")
				.mockRejectedValue(new Error("embedding unavailable"));
			try {
				const sessionId = insertTestSession(store.db);
				expect(() =>
					store.remember(sessionId, "feature", "Resilient title", "Resilient body"),
				).not.toThrow();
			} finally {
				storeVectorsSpy.mockRestore();
			}
		});

		it("does not launch vector writes from inside an open transaction", () => {
			const storeVectorsSpy = vi.spyOn(vectors, "storeVectors").mockResolvedValue();
			try {
				const sessionId = insertTestSession(store.db);
				store.db.transaction(() => {
					store.remember(sessionId, "feature", "Tx title", "Tx body");
					expect(storeVectorsSpy).not.toHaveBeenCalled();
				})();
				expect(storeVectorsSpy).not.toHaveBeenCalled();
			} finally {
				storeVectorsSpy.mockRestore();
			}
		});

		it("returns the existing id for same-session duplicate normalized titles", () => {
			const sessionId = insertTestSession(store.db);
			const firstId = store.remember(
				sessionId,
				"feature",
				"PR #123 Sync pass orchestrator ported to TypeScript",
				"Original body",
			);
			const duplicateId = store.remember(
				sessionId,
				"feature",
				"Sync pass orchestrator ported to TypeScript",
				"Duplicate body",
			);

			expect(duplicateId).toBe(firstId);
			const count = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as {
				count: number;
			};
			expect(count.count).toBe(1);
		});

		it("returns same-session duplicates when legacy scope is missing", () => {
			const nullScopeSessionId = insertTestSession(store.db);
			const nullScopeId = store.remember(
				nullScopeSessionId,
				"feature",
				"Legacy null scope title",
				"Original body",
			);
			store.db.prepare("UPDATE memory_items SET scope_id = NULL WHERE id = ?").run(nullScopeId);

			const nullScopeDuplicateId = store.remember(
				nullScopeSessionId,
				"feature",
				"Legacy null scope title",
				"Duplicate body",
			);

			const emptyScopeSessionId = insertTestSession(store.db);
			const emptyScopeId = store.remember(
				emptyScopeSessionId,
				"feature",
				"Legacy empty scope title",
				"Original body",
			);
			store.db.prepare("UPDATE memory_items SET scope_id = '' WHERE id = ?").run(emptyScopeId);

			const emptyScopeDuplicateId = store.remember(
				emptyScopeSessionId,
				"feature",
				"Legacy empty scope title",
				"Duplicate body",
			);

			expect(nullScopeDuplicateId).toBe(nullScopeId);
			expect(emptyScopeDuplicateId).toBe(emptyScopeId);
			const count = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as {
				count: number;
			};
			expect(count.count).toBe(2);
		});

		it("logs same-session dedup hits when CODEMEM_DEBUG=1", () => {
			process.env.CODEMEM_DEBUG = "1";
			const stderrSpy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
			try {
				const sessionId = insertTestSession(store.db);
				const firstId = store.remember(sessionId, "feature", "Same session title", "Original body");
				const duplicateId = store.remember(
					sessionId,
					"feature",
					"Same session title",
					"Duplicate body",
				);

				expect(duplicateId).toBe(firstId);
				expect(stderrSpy).toHaveBeenCalledWith(
					expect.stringContaining("[codemem] memory dedup hit scope=same_session"),
				);
			} finally {
				stderrSpy.mockRestore();
			}
		});

		it("returns the existing id for same-session duplicates when normalization strips the title", () => {
			const sessionId = insertTestSession(store.db);
			const firstId = store.remember(sessionId, "feature", "PR #77", "Original body");
			const duplicateId = store.remember(sessionId, "feature", "PR #77", "Duplicate body");

			expect(duplicateId).toBe(firstId);
			const count = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as {
				count: number;
			};
			expect(count.count).toBe(1);
		});

		it("returns the existing id for cross-session duplicates within the default window", () => {
			const sessionA = insertTestSession(store.db);
			const sessionB = insertTestSession(store.db);
			const firstId = store.remember(
				sessionA,
				"discovery",
				"Issue #649 Context inspector stale query state",
				"Original body",
				0.9,
			);
			const duplicateId = store.remember(
				sessionB,
				"discovery",
				"Context inspector stale query state",
				"Duplicate body",
				0.5,
			);

			expect(duplicateId).toBe(firstId);
			const count = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as {
				count: number;
			};
			expect(count.count).toBe(1);
		});

		it("does not dedup duplicate titles across different scopes", () => {
			const sessionA = insertTestSession(store.db);
			const sessionB = insertTestSession(store.db);
			store.db
				.prepare("UPDATE sessions SET cwd = ?, project = ? WHERE id = ?")
				.run("/work/acme/service", "service", sessionA);
			store.db
				.prepare("UPDATE sessions SET cwd = ?, project = ? WHERE id = ?")
				.run("/oss/codemem", "codemem", sessionB);
			store.db
				.prepare(
					`INSERT INTO project_scope_mappings(
						workspace_identity, project_pattern, scope_id, priority, source, created_at, updated_at
					 ) VALUES (?, ?, ?, ?, ?, ?, ?), (?, ?, ?, ?, ?, ?, ?)`,
				)
				.run(
					"/work/acme/service",
					"/work/acme/*",
					"acme-work",
					10,
					"user",
					"2026-05-01T00:00:00Z",
					"2026-05-01T00:00:00Z",
					"/oss/codemem",
					"/oss/*",
					"oss-codemem",
					10,
					"user",
					"2026-05-01T00:00:00Z",
					"2026-05-01T00:00:00Z",
				);

			const firstId = store.remember(sessionA, "discovery", "Shared title", "Original body", 0.9);
			const secondId = store.remember(sessionB, "discovery", "Shared title", "Duplicate body", 0.5);

			expect(secondId).not.toBe(firstId);
			const scopes = store.db
				.prepare("SELECT scope_id FROM memory_items ORDER BY id")
				.all() as Array<{ scope_id: string | null }>;
			expect(scopes).toEqual([{ scope_id: "acme-work" }, { scope_id: "oss-codemem" }]);
		});

		it("logs cross-session dedup hits when CODEMEM_DEBUG=1", () => {
			process.env.CODEMEM_DEBUG = "1";
			const stderrSpy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
			try {
				const sessionA = insertTestSession(store.db);
				const sessionB = insertTestSession(store.db);
				const firstId = store.remember(
					sessionA,
					"discovery",
					"Cross session title",
					"Original body",
					0.9,
				);
				const duplicateId = store.remember(
					sessionB,
					"discovery",
					"Cross session title",
					"Duplicate body",
					0.5,
				);

				expect(duplicateId).toBe(firstId);
				expect(stderrSpy).toHaveBeenCalledWith(
					expect.stringContaining("[codemem] memory dedup hit scope=cross_session"),
				);
			} finally {
				stderrSpy.mockRestore();
			}
		});

		it("inserts a new row for cross-session duplicates outside the dedup window", () => {
			const sessionA = insertTestSession(store.db);
			const sessionB = insertTestSession(store.db);
			const firstId = store.remember(
				sessionA,
				"discovery",
				"Duplicate title",
				"Original body",
				0.9,
			);
			store.db
				.prepare("UPDATE memory_items SET created_at = ?, updated_at = ? WHERE id = ?")
				.run("2026-01-01T00:00:00.000Z", "2026-01-01T00:00:00.000Z", firstId);

			const secondId = store.remember(sessionB, "discovery", "Duplicate title", "New body", 0.5);

			expect(secondId).not.toBe(firstId);
			const count = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as {
				count: number;
			};
			expect(count.count).toBe(2);
		});

		it("disables cross-session dedup when the window env var is 0", () => {
			store.close();
			process.env.CODEMEM_MEMORY_CROSS_SESSION_DEDUP_WINDOW_MS = "0";
			store = openTestMemoryStore(dbPath);

			const sessionA = insertTestSession(store.db);
			const sessionB = insertTestSession(store.db);
			const firstId = store.remember(
				sessionA,
				"discovery",
				"Duplicate title",
				"Original body",
				0.9,
			);
			const secondId = store.remember(sessionB, "discovery", "Duplicate title", "New body", 0.5);

			expect(secondId).not.toBe(firstId);
			const count = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as {
				count: number;
			};
			expect(count.count).toBe(2);
		});

		it("clamps absurdly large cross-session dedup windows instead of throwing", () => {
			store.close();
			process.env.CODEMEM_MEMORY_CROSS_SESSION_DEDUP_WINDOW_MS = "9000000000000000";
			store = openTestMemoryStore(dbPath);

			const sessionA = insertTestSession(store.db);
			const sessionB = insertTestSession(store.db);

			expect(() => {
				store.remember(sessionA, "discovery", "Large window title", "Original body", 0.9);
				store.remember(sessionB, "discovery", "Large window title", "Duplicate body", 0.5);
			}).not.toThrow();
		});

		it("does not dedup across different kinds even with the same normalized title", () => {
			const sessionA = insertTestSession(store.db);
			const sessionB = insertTestSession(store.db);
			const firstId = store.remember(
				sessionA,
				"discovery",
				"Duplicate title",
				"Original body",
				0.9,
			);
			const secondId = store.remember(
				sessionB,
				"session_summary",
				"Duplicate title",
				"Summary body",
				0.5,
			);

			expect(secondId).not.toBe(firstId);
			const count = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as {
				count: number;
			};
			expect(count.count).toBe(2);
		});

		it("intentionally prefers the first row when normalized titles match but bodies differ", () => {
			const sessionA = insertTestSession(store.db);
			const sessionB = insertTestSession(store.db);
			const firstId = store.remember(
				sessionA,
				"discovery",
				"PR #321 observer narrative persistence",
				"First body",
				0.9,
			);
			const duplicateId = store.remember(
				sessionB,
				"discovery",
				"Observer narrative persistence",
				"Second body with different details",
				0.4,
			);

			expect(duplicateId).toBe(firstId);
			const row = store.get(firstId);
			expect(row?.body_text).toBe("First body");
		});

		it("does not log dedup hits by default", () => {
			const stderrSpy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
			try {
				const sessionId = insertTestSession(store.db);
				store.remember(sessionId, "feature", "Silent dedup title", "Original body");
				store.remember(sessionId, "feature", "Silent dedup title", "Duplicate body");

				expect(stderrSpy).not.toHaveBeenCalled();
			} finally {
				stderrSpy.mockRestore();
			}
		});

		it("populates memory_file_refs for files_read and files_modified", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "File refs test", "Body", 0.5, [], {
				files_read: ["src/auth.ts", "src/config.ts"],
				files_modified: ["src/auth.ts"],
			});

			const fileRefs = store.db
				.prepare("SELECT * FROM memory_file_refs WHERE memory_id = ? ORDER BY file_path, relation")
				.all(memId) as Array<{ memory_id: number; file_path: string; relation: string }>;

			expect(fileRefs).toHaveLength(3);
			expect(fileRefs).toContainEqual({
				memory_id: memId,
				file_path: "src/auth.ts",
				relation: "read",
			});
			expect(fileRefs).toContainEqual({
				memory_id: memId,
				file_path: "src/config.ts",
				relation: "read",
			});
			expect(fileRefs).toContainEqual({
				memory_id: memId,
				file_path: "src/auth.ts",
				relation: "modified",
			});
		});

		it("populates memory_concept_refs with normalized concepts", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Concept refs test", "Body", 0.5, [], {
				concepts: ["Auth", "security", " oauth "],
			});

			const conceptRefs = store.db
				.prepare("SELECT * FROM memory_concept_refs WHERE memory_id = ? ORDER BY concept")
				.all(memId) as Array<{ memory_id: number; concept: string }>;

			expect(conceptRefs).toHaveLength(3);
			expect(conceptRefs).toContainEqual({ memory_id: memId, concept: "auth" });
			expect(conceptRefs).toContainEqual({ memory_id: memId, concept: "security" });
			expect(conceptRefs).toContainEqual({ memory_id: memId, concept: "oauth" });
		});

		it("creates no ref rows when files and concepts are null or empty", () => {
			const sessionId = insertTestSession(store.db);
			// No metadata at all
			const memId1 = store.remember(sessionId, "discovery", "No refs test 1", "Body");
			// Empty arrays
			const memId2 = store.remember(sessionId, "discovery", "No refs test 2", "Body", 0.5, [], {
				files_read: [],
				files_modified: [],
				concepts: [],
			});

			for (const memId of [memId1, memId2]) {
				const fileRefs = store.db
					.prepare("SELECT * FROM memory_file_refs WHERE memory_id = ?")
					.all(memId);
				const conceptRefs = store.db
					.prepare("SELECT * FROM memory_concept_refs WHERE memory_id = ?")
					.all(memId);
				expect(fileRefs).toHaveLength(0);
				expect(conceptRefs).toHaveLength(0);
			}
		});

		it("rolls back memory_items insert when ref population fails", () => {
			const sessionId = insertTestSession(store.db);

			// Sabotage the memory_file_refs table so INSERT OR IGNORE still throws
			store.db.exec("DROP TABLE memory_file_refs");

			expect(() =>
				store.remember(sessionId, "discovery", "Rollback test", "Body", 0.5, [], {
					files_read: ["src/oops.ts"],
				}),
			).toThrow();

			// The memory_items insert should have been rolled back
			const row = store.db
				.prepare("SELECT id FROM memory_items WHERE title = ?")
				.get("Rollback test");
			expect(row).toBeUndefined();
		});
	});

	// -- forget --------------------------------------------------------------

	describe("memoryOwnedBySelf", () => {
		it("returns false (without throwing) when sync_peers table is unavailable", () => {
			store.db.prepare("DROP TABLE IF EXISTS sync_peers").run();

			expect(() =>
				store.memoryOwnedBySelf({
					actor_id: "legacy-sync:peer-missing",
					origin_device_id: "peer-missing",
					metadata: {},
				}),
			).not.toThrow();
			expect(
				store.memoryOwnedBySelf({
					actor_id: "legacy-sync:peer-missing",
					origin_device_id: "peer-missing",
					metadata: {},
				}),
			).toBe(false);
		});

		it("returns true for claimed same-actor peer origin_device_id", () => {
			store.db
				.prepare(
					"INSERT INTO sync_peers(peer_device_id, actor_id, claimed_local_actor, created_at) VALUES (?, ?, ?, ?)",
				)
				.run("peer-claimed-1", store.actorId, 1, "2026-01-01T00:00:00Z");

			expect(
				store.memoryOwnedBySelf({
					actor_id: null,
					origin_device_id: "peer-claimed-1",
					metadata: {},
				}),
			).toBe(true);
		});

		it("returns true for legacy-sync actor IDs tied to claimed peers", () => {
			store.db
				.prepare(
					"INSERT INTO sync_peers(peer_device_id, actor_id, claimed_local_actor, created_at) VALUES (?, ?, ?, ?)",
				)
				.run("peer-claimed-2", store.actorId, 1, "2026-01-01T00:00:00Z");

			expect(
				store.memoryOwnedBySelf({
					actor_id: "legacy-sync:peer-claimed-2",
					origin_device_id: null,
					metadata: {},
				}),
			).toBe(true);
		});

		it("reads actor/origin ownership from metadata when top-level fields are absent", () => {
			store.db
				.prepare(
					"INSERT INTO sync_peers(peer_device_id, actor_id, claimed_local_actor, created_at) VALUES (?, ?, ?, ?)",
				)
				.run("peer-claimed-3", store.actorId, 1, "2026-01-01T00:00:00Z");

			expect(
				store.memoryOwnedBySelf({
					metadata: {
						actor_id: "legacy-sync:peer-claimed-3",
						origin_device_id: "peer-claimed-3",
					},
				}),
			).toBe(true);
		});

		it("memoryOwnedBySelf reflects sync_peers changes immediately (no caching)", () => {
			// Authorization-critical callers (forgetMemory, setMemoryVisibility,
			// etc.) call memoryOwnedBySelf to decide whether a write is
			// allowed. The result must reflect the latest sync_peers state
			// on every call — caching here would mean an unclaimed peer's
			// writes could still be authorized inside a stale window.
			expect(
				store.memoryOwnedBySelf({
					origin_device_id: "peer-fresh",
					metadata: {},
				}),
			).toBe(false);

			store.db
				.prepare(
					"INSERT INTO sync_peers(peer_device_id, actor_id, claimed_local_actor, created_at) VALUES (?, ?, ?, ?)",
				)
				.run("peer-fresh", store.actorId, 1, "2026-01-01T00:00:00Z");

			expect(
				store.memoryOwnedBySelf({
					origin_device_id: "peer-fresh",
					metadata: {},
				}),
			).toBe(true);

			store.db.prepare("DELETE FROM sync_peers WHERE peer_device_id = ?").run("peer-fresh");

			expect(
				store.memoryOwnedBySelf({
					origin_device_id: "peer-fresh",
					metadata: {},
				}),
			).toBe(false);
		});

		it("buildOwnershipPredicate snapshots sync_peers once for hot-loop callers", () => {
			store.db
				.prepare(
					"INSERT INTO sync_peers(peer_device_id, actor_id, claimed_local_actor, created_at) VALUES (?, ?, ?, ?)",
				)
				.run("peer-snapshot", store.actorId, 1, "2026-01-01T00:00:00Z");

			const predicate = store.buildOwnershipPredicate();
			const spy = vi.spyOn(store, "sameActorPeerIds");
			try {
				for (let index = 0; index < 50; index += 1) {
					predicate({ origin_device_id: "peer-snapshot", metadata: {} });
				}
				expect(spy).not.toHaveBeenCalled();
			} finally {
				spy.mockRestore();
			}
		});

		it("buildOwnershipPredicate snapshot does not observe later peer changes", () => {
			const predicate = store.buildOwnershipPredicate();

			store.db
				.prepare(
					"INSERT INTO sync_peers(peer_device_id, actor_id, claimed_local_actor, created_at) VALUES (?, ?, ?, ?)",
				)
				.run("peer-late", store.actorId, 1, "2026-01-01T00:00:00Z");

			// Frozen-snapshot semantics: each request rebuilds the predicate,
			// so callers that hold one across requests intentionally trade
			// freshness for the per-loop perf win. memoryOwnedBySelf remains
			// the always-fresh path for write-side authorization.
			expect(predicate({ origin_device_id: "peer-late", metadata: {} })).toBe(false);
			expect(store.memoryOwnedBySelf({ origin_device_id: "peer-late", metadata: {} })).toBe(true);
		});
	});

	describe("rememberTrusted", () => {
		const repositoryIdentity = `repo-v1:sha256:${"a".repeat(64)}`;
		const trustedWrite = (
			title: string,
			sensitivity: "eligible" | "local_only" | "private" | "secret",
			identity: string | null,
		) => {
			const sessionId = insertTestSession(store.db);
			const memoryId = store.rememberTrusted(
				sessionId,
				"discovery",
				title,
				"Body",
				0.5,
				undefined,
				undefined,
				{ sensitivity, repositoryIdentity: identity },
			);
			return store.db
				.prepare("SELECT sensitivity, repository_identity FROM memory_items WHERE id = ?")
				.get(memoryId);
		};

		it("defaults a restricted write without repository identity to secret", () => {
			// Gate fires: restricted + no identity can never satisfy a boundary,
			// so it must not be stored as a write-only local_only/private row.
			expect(trustedWrite("No repo local", "local_only", null)).toEqual({
				sensitivity: "secret",
				repository_identity: null,
			});
			expect(trustedWrite("No repo private", "private", null)).toEqual({
				sensitivity: "secret",
				repository_identity: null,
			});
			// Gate passes: the pair invariant holds with an identity present.
			expect(trustedWrite("Paired local", "local_only", repositoryIdentity)).toEqual({
				sensitivity: "local_only",
				repository_identity: repositoryIdentity,
			});
			expect(trustedWrite("Plain eligible", "eligible", null)).toEqual({
				sensitivity: "eligible",
				repository_identity: null,
			});
		});
	});

	describe("forget", () => {
		it("soft-deletes an existing memory", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "To Delete", "Body");

			store.forget(memId);

			const row = store.get(memId);
			expect(row).not.toBeNull();
			expect(row?.active).toBe(0);
			expect(row?.deleted_at).toBeTruthy();
			expect(row?.rev).toBe(2); // was 1, bumped to 2
		});

		it("updates metadata_json with clock_device_id", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "To Delete", "Body");
			store.forget(memId);

			const row = store.get(memId);
			expect(row?.metadata_json.clock_device_id).toBe(store.deviceId);
		});

		it("is a no-op for non-existent memory", () => {
			expect(() => store.forget(99999)).not.toThrow();
		});
	});

	// -- recent --------------------------------------------------------------

	describe("recent", () => {
		it("returns active memories ordered by created_at DESC", () => {
			const sessionId = insertTestSession(store.db);
			// Insert with explicit timestamps to guarantee ordering
			const base = "2026-01-01T00:00:0";
			for (const [i, kind] of (["discovery", "feature", "bugfix"] as const).entries()) {
				store.db
					.prepare(
						`INSERT INTO memory_items(session_id, kind, title, body_text, confidence,
					tags_text, active, created_at, updated_at, metadata_json, rev, scope_id)
					VALUES (?, ?, ?, ?, 0.5, '', 1, ?, ?, '{}', 1, 'local-default')`,
					)
					.run(sessionId, kind, `Item ${i}`, `Body ${i}`, `${base}${i}Z`, `${base}${i}Z`);
			}

			const results = store.recent(10);
			expect(results).toHaveLength(3);
			// Newest first (bugfix at :02, feature at :01, discovery at :00)
			expect(results[0]?.kind).toBe("bugfix");
			expect(results[2]?.kind).toBe("discovery");
		});

		it("excludes soft-deleted memories", () => {
			const sessionId = insertTestSession(store.db);
			const id1 = store.remember(sessionId, "discovery", "Keep", "Body");
			const id2 = store.remember(sessionId, "discovery", "Delete", "Body");
			store.forget(id2);

			const results = store.recent(10);
			expect(results).toHaveLength(1);
			expect(results[0].id).toBe(id1);
		});

		it("respects limit and offset", () => {
			const sessionId = insertTestSession(store.db);
			for (let i = 0; i < 5; i++) {
				store.remember(sessionId, "discovery", `Item ${i}`, `Body ${i}`);
			}

			const page1 = store.recent(2, null, 0);
			const page2 = store.recent(2, null, 2);
			expect(page1).toHaveLength(2);
			expect(page2).toHaveLength(2);
			expect(page1[0].id).not.toBe(page2[0].id);
		});

		it("filters by kind", () => {
			const sessionId = insertTestSession(store.db);
			store.remember(sessionId, "discovery", "D1", "Body");
			store.remember(sessionId, "feature", "F1", "Body");
			store.remember(sessionId, "discovery", "D2", "Body");

			const results = store.recent(10, { kind: "discovery" });
			expect(results).toHaveLength(2);
			for (const r of results) {
				expect(r.kind).toBe("discovery");
			}
		});

		it("treats claimed same-actor peer rows as mine for SQL ownership filters", () => {
			const sessionId = insertTestSession(store.db);
			const now = "2026-01-01T00:00:00Z";
			store.db
				.prepare(
					"INSERT INTO sync_peers(peer_device_id, actor_id, claimed_local_actor, created_at) VALUES (?, ?, ?, ?)",
				)
				.run("peer-claimed", store.actorId, 1, now);

			const insertMemory = (
				title: string,
				actorId: string | null,
				originDeviceId: string | null,
				metadata: Record<string, unknown> = {},
			) => {
				const info = store.db
					.prepare(
						`INSERT INTO memory_items(
							session_id, kind, title, body_text, confidence, tags_text, active,
							created_at, updated_at, metadata_json, rev, scope_id, actor_id, origin_device_id
						 ) VALUES (?, 'discovery', ?, 'Body', 0.5, '', 1, ?, ?, ?, 1, 'local-default', ?, ?)`,
					)
					.run(sessionId, title, now, now, JSON.stringify(metadata), actorId, originDeviceId);
				return Number(info.lastInsertRowid);
			};

			const claimedDeviceId = insertMemory("Claimed peer device", null, "peer-claimed");
			const legacyActorId = insertMemory(
				"Claimed peer legacy actor",
				"legacy-sync:peer-claimed",
				"unknown-origin",
			);
			const metadataDeviceId = insertMemory("Claimed metadata peer device", null, null, {
				origin_device_id: "peer-claimed",
			});
			const metadataLegacyActorId = insertMemory("Claimed metadata legacy actor", null, null, {
				actor_id: "legacy-sync:peer-claimed",
			});
			const otherId = insertMemory("Other peer", "local:other-peer", "other-peer");

			const mineIds = store.recent(10, { ownership_scope: "mine" }).map((item) => item.id);
			expect(mineIds).toContain(claimedDeviceId);
			expect(mineIds).toContain(legacyActorId);
			expect(mineIds).toContain(metadataDeviceId);
			expect(mineIds).toContain(metadataLegacyActorId);
			expect(mineIds).not.toContain(otherId);

			const theirsIds = store.recent(10, { ownership_scope: "theirs" }).map((item) => item.id);
			expect(theirsIds).not.toContain(claimedDeviceId);
			expect(theirsIds).not.toContain(legacyActorId);
			expect(theirsIds).not.toContain(metadataDeviceId);
			expect(theirsIds).not.toContain(metadataLegacyActorId);
			expect(theirsIds).toContain(otherId);
		});

		it("intersects explicit scope filters with local authorization", () => {
			grantScopeToLocalDevice("authorized-team");
			insertCoordinatorScope("unauthorized-team");
			const visibleId = insertScopedMemory("authorized-team", "Authorized recent");
			const hiddenId = insertScopedMemory("unauthorized-team", "Unauthorized recent");

			const defaultRecentIds = store.recent(10).map((item) => item.id);
			expect(defaultRecentIds).toContain(visibleId);
			expect(defaultRecentIds).not.toContain(hiddenId);

			expect(store.recent(10, { include_scope_ids: ["unauthorized-team"] })).toEqual([]);
			expect(store.recent(10, { scope_id: "unauthorized-team" })).toEqual([]);
			expect(
				store.recent(10, { include_scope_ids: ["authorized-team"] }).map((item) => item.id),
			).toContain(visibleId);
			expect(store.recent(10, { scope_id: "authorized-team" }).map((item) => item.id)).toContain(
				visibleId,
			);
		});

		it("treats legacy null/blank scope rows as locally visible", () => {
			const sessionId = insertTestSession(store.db);
			const now = "2026-01-01T00:00:00Z";
			const insertWithScope = (title: string, scopeValue: string | null): number => {
				const info = store.db
					.prepare(
						`INSERT INTO memory_items(
							session_id, kind, title, body_text, confidence, tags_text, active,
							created_at, updated_at, metadata_json, rev, scope_id
						 ) VALUES (?, 'discovery', ?, 'legacy body', 0.5, '', 1, ?, ?, '{}', 1, ?)`,
					)
					.run(sessionId, title, now, now, scopeValue);
				return Number(info.lastInsertRowid);
			};
			const nullScopeId = insertWithScope("Legacy null scope", null);
			const blankScopeId = insertWithScope("Legacy blank scope", "");

			const recentIds = store.recent(10).map((item) => item.id);
			expect(recentIds).toContain(nullScopeId);
			expect(recentIds).toContain(blankScopeId);

			const searchIds = store.search("legacy", 10).map((item) => item.id);
			expect(searchIds).toContain(nullScopeId);
			expect(searchIds).toContain(blankScopeId);

			expect(store.get(nullScopeId)?.title).toBe("Legacy null scope");
			expect(store.get(blankScopeId)?.title).toBe("Legacy blank scope");
		});
	});

	// -- recentByKinds -------------------------------------------------------

	describe("recentByKinds", () => {
		it("filters by multiple kinds", () => {
			const sessionId = insertTestSession(store.db);
			store.remember(sessionId, "discovery", "D1", "Body");
			store.remember(sessionId, "feature", "F1", "Body");
			store.remember(sessionId, "bugfix", "B1", "Body");
			store.remember(sessionId, "refactor", "R1", "Body");

			const results = store.recentByKinds(["discovery", "bugfix"]);
			expect(results).toHaveLength(2);
			const kinds = results.map((r) => r.kind);
			expect(kinds).toContain("discovery");
			expect(kinds).toContain("bugfix");
		});

		it("returns empty array for empty kinds list", () => {
			const results = store.recentByKinds([]);
			expect(results).toEqual([]);
		});

		it("intersects scope filters with local authorization", () => {
			grantScopeToLocalDevice("authorized-team");
			insertCoordinatorScope("unauthorized-team");
			const visibleId = insertScopedMemory("authorized-team", "Authorized by-kind");
			const hiddenId = insertScopedMemory("unauthorized-team", "Unauthorized by-kind");

			const defaultIds = store.recentByKinds(["discovery"], 10).map((item) => item.id);
			expect(defaultIds).toContain(visibleId);
			expect(defaultIds).not.toContain(hiddenId);

			expect(store.recentByKinds(["discovery"], 10, { scope_id: "unauthorized-team" })).toEqual([]);
			expect(
				store
					.recentByKinds(["discovery"], 10, { include_scope_ids: ["authorized-team"] })
					.map((item) => item.id),
			).toContain(visibleId);
		});
	});

	// -- stats ---------------------------------------------------------------

	describe("stats", () => {
		it("returns a structured stats object", () => {
			const sessionId = insertTestSession(store.db);
			store.remember(sessionId, "discovery", "Title", "Body");

			const result = store.stats();
			expect(result.database).toBeDefined();
			expect(result.database.path).toBe(dbPath);
			expect(result.database.size_bytes).toBeGreaterThan(0);
			expect(result.database.sessions).toBe(1);
			expect(result.database.memory_items).toBe(1);
			expect(result.database.active_memory_items).toBe(1);
			expect(result.database.artifacts).toBe(0);
			expect(result.database.raw_events).toBe(0);
		});

		it("counts inactive memories in total but not active", () => {
			const sessionId = insertTestSession(store.db);
			const _id1 = store.remember(sessionId, "discovery", "Active", "Body");
			const id2 = store.remember(sessionId, "discovery", "Deleted", "Body");
			store.forget(id2);

			const result = store.stats();
			expect(result.database.memory_items).toBe(2);
			expect(result.database.active_memory_items).toBe(1);
		});

		it("excludes memories outside locally authorized scopes from memory stats", () => {
			grantScopeToLocalDevice("authorized-team");
			insertCoordinatorScope("unauthorized-team");
			insertScopedMemory("authorized-team", "Authorized stats memory");
			insertScopedMemory("unauthorized-team", "Unauthorized stats memory");

			const result = store.stats();
			expect(result.database.sessions).toBe(1);
			expect(result.database.memory_items).toBe(1);
			expect(result.database.active_memory_items).toBe(1);
		});

		it("handles memory_vectors count failures without crashing", () => {
			const sessionId = insertTestSession(store.db);
			store.remember(sessionId, "discovery", "Vector test", "Body");
			store.db.exec("CREATE TABLE IF NOT EXISTS memory_vectors(id INTEGER)");

			const originalPrepare = store.db.prepare.bind(store.db);
			(store.db as unknown as { prepare: typeof store.db.prepare }).prepare = ((
				statement: string,
			) => {
				if (statement.includes("FROM memory_vectors")) {
					throw new Error("no such module: vec0");
				}
				return originalPrepare(statement);
			}) as typeof store.db.prepare;

			try {
				const result = store.stats();
				expect(result.database.vector_coverage).toBe(0);
			} finally {
				(store.db as unknown as { prepare: typeof store.db.prepare }).prepare = originalPrepare;
			}
		});
	});

	// -- usageAggregate ------------------------------------------------------

	describe("usageAggregate", () => {
		function insertUsageEvent(
			sessionId: number | null,
			event: string,
			tokensRead: number,
			tokensWritten: number,
			tokensSaved: number | null,
			createdAt = "2026-03-26T23:30:00Z",
		): void {
			store.db
				.prepare(
					`INSERT INTO usage_events(session_id, event, tokens_read, tokens_written, tokens_saved, created_at, metadata_json)
					 VALUES (?, ?, ?, ?, ?, ?, '{}')`,
				)
				.run(sessionId, event, tokensRead, tokensWritten, tokensSaved, createdAt);
		}

		it("groups by event and sums tokens with tokens_saved NULL coalesced to 0", () => {
			const sessionId = insertTestSession(store.db);
			insertUsageEvent(sessionId, "pack", 100, 10, 5);
			insertUsageEvent(sessionId, "pack", 200, 20, null);
			insertUsageEvent(sessionId, "search", 30, 3, 7);

			const rows = store.usageAggregate();
			const byEvent = new Map(rows.map((row) => [row.event, row]));
			expect(byEvent.get("pack")).toEqual({
				event: "pack",
				count: 2,
				tokens_read: 300,
				tokens_written: 30,
				// NULL tokens_saved on the second pack contributes 0.
				tokens_saved: 5,
			});
			expect(byEvent.get("search")).toEqual({
				event: "search",
				count: 1,
				tokens_read: 30,
				tokens_written: 3,
				tokens_saved: 7,
			});
		});

		it("restricts the aggregate to a project when a non-empty filter is given", () => {
			const codememSession = insertTestSession(store.db);
			store.db
				.prepare("UPDATE sessions SET project = ? WHERE id = ?")
				.run("codemem", codememSession);
			const otherSession = insertTestSession(store.db);
			store.db.prepare("UPDATE sessions SET project = ? WHERE id = ?").run("other", otherSession);
			insertUsageEvent(codememSession, "pack", 100, 10, 5);
			insertUsageEvent(otherSession, "pack", 1000, 100, 50);

			const filtered = store.usageAggregate("codemem");
			expect(filtered).toEqual([
				{ event: "pack", count: 1, tokens_read: 100, tokens_written: 10, tokens_saved: 5 },
			]);

			// Empty-string filter behaves like the unfiltered (global) variant.
			const globalViaEmpty = store.usageAggregate("");
			const global = store.usageAggregate();
			expect(globalViaEmpty).toEqual(global);
			expect(global).toEqual([
				{ event: "pack", count: 2, tokens_read: 1100, tokens_written: 110, tokens_saved: 55 },
			]);
		});

		it("returns an empty array when there are no usage events", () => {
			expect(store.usageAggregate()).toEqual([]);
			expect(store.usageAggregate("codemem")).toEqual([]);
		});

		it("backs store.stats().usage with the same unfiltered values", () => {
			const sessionId = insertTestSession(store.db);
			insertUsageEvent(sessionId, "pack", 100, 10, 5);
			insertUsageEvent(sessionId, "pack", 200, 20, null);
			insertUsageEvent(sessionId, "search", 30, 3, 7);

			const usage = store.stats().usage;
			// stats() sorts events by count DESC.
			expect(usage.events).toEqual([
				{ event: "pack", count: 2, tokens_read: 300, tokens_written: 30, tokens_saved: 5 },
				{ event: "search", count: 1, tokens_read: 30, tokens_written: 3, tokens_saved: 7 },
			]);
			expect(usage.totals).toEqual({
				events: 3,
				tokens_read: 330,
				tokens_written: 33,
				tokens_saved: 12,
			});
		});
	});

	// -- updateMemoryVisibility ----------------------------------------------

	describe("updateMemoryVisibility", () => {
		it("updates visibility to private with actor-scoped workspace_id", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			const updated = store.updateMemoryVisibility(memId, "private");
			expect(updated.visibility).toBe("private");
			expect(updated.workspace_kind).toBe("personal");
			// Python uses personal:${actor_id} where actor_id = local:${device_id}
			expect(updated.workspace_id).toBe(`personal:${store.actorId}`);
		});

		it("updates metadata_json with clock_device_id on visibility change", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			const updated = store.updateMemoryVisibility(memId, "shared");
			expect(updated.metadata_json.clock_device_id).toBe(store.deviceId);
			expect(updated.metadata_json.visibility).toBe("shared");
		});

		it("updates visibility to shared", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			const updated = store.updateMemoryVisibility(memId, "shared");
			expect(updated.visibility).toBe("shared");
			expect(updated.workspace_kind).toBe("shared");
		});

		it("bumps rev on visibility change", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			const original = store.get(memId);
			const updated = store.updateMemoryVisibility(memId, "private");
			expect(updated.rev).toBe((original?.rev as number) + 1);
		});

		it("throws for invalid visibility", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			expect(() => store.updateMemoryVisibility(memId, "invalid")).toThrow(
				/visibility must be private or shared/,
			);
		});

		it("throws for non-existent memory", () => {
			expect(() => store.updateMemoryVisibility(99999, "shared")).toThrow(/memory not found/);
		});

		it("throws for inactive memory", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");
			store.forget(memId);

			expect(() => store.updateMemoryVisibility(memId, "shared")).toThrow(/memory not found/);
		});

		it("throws for memory owned by another device", () => {
			const sessionId = insertTestSession(store.db);
			// Insert a memory with a different origin_device_id
			store.db
				.prepare(
					`INSERT INTO memory_items(session_id, kind, title, body_text, confidence,
					tags_text, active, created_at, updated_at, metadata_json,
					origin_device_id, rev, scope_id)
					VALUES (?, 'discovery', 'Foreign', 'Body', 0.5, '', 1, ?, ?, '{}', 'other-device', 1, 'local-default')`,
				)
				.run(sessionId, new Date().toISOString(), new Date().toISOString());
			const foreignId = Number(
				(store.db.prepare("SELECT last_insert_rowid() as id").get() as { id: number }).id,
			);

			expect(() => store.updateMemoryVisibility(foreignId, "private")).toThrow(
				/not owned by this device/,
			);
		});
	});

	// -- close ---------------------------------------------------------------

	// -- moveMemoryProject ---------------------------------------------------

	describe("moveMemoryProject", () => {
		function sessionProject(db: MemoryStore["db"], sessionId: number): string | null {
			const row = db.prepare("SELECT project FROM sessions WHERE id = ?").get(sessionId) as
				| { project: string | null }
				| undefined;
			return row?.project ?? null;
		}

		it("updates the parent session's project to the trimmed value", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			const result = store.moveMemoryProject(memId, "  new-project  ");
			expect(result.project).toBe("new-project");
			expect(result.session_id).toBe(sessionId);
			expect(sessionProject(store.db, sessionId)).toBe("new-project");
		});

		it("reports the number of sibling memories that moved with the session", () => {
			const sessionId = insertTestSession(store.db);
			const memA = store.remember(sessionId, "discovery", "Title A", "Body");
			store.remember(sessionId, "discovery", "Title B", "Body");
			store.remember(sessionId, "discovery", "Title C", "Body");

			const result = store.moveMemoryProject(memA, "other");
			expect(result.moved_memory_count).toBe(3);
		});

		it("excludes inactive siblings from the moved_memory_count", () => {
			const sessionId = insertTestSession(store.db);
			const memA = store.remember(sessionId, "discovery", "Title A", "Body");
			const memB = store.remember(sessionId, "discovery", "Title B", "Body");
			store.forget(memB);

			const result = store.moveMemoryProject(memA, "other");
			expect(result.moved_memory_count).toBe(1);
		});

		it("throws when project is empty or whitespace", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");

			expect(() => store.moveMemoryProject(memId, "")).toThrow(/non-empty/);
			expect(() => store.moveMemoryProject(memId, "   ")).toThrow(/non-empty/);
		});

		it("throws when the memory is not found", () => {
			expect(() => store.moveMemoryProject(99999, "anything")).toThrow(/memory not found/);
		});

		it("throws when the memory is inactive", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");
			store.forget(memId);

			expect(() => store.moveMemoryProject(memId, "new")).toThrow(/memory not found/);
		});

		it("recognizes self-ownership from metadata_json on legacy rows with null top-level columns", () => {
			const sessionId = insertTestSession(store.db);
			const memId = store.remember(sessionId, "discovery", "Title", "Body");
			// Simulate a legacy/imported row: clear top-level actor_id /
			// origin_device_id, but stash the same values inside metadata_json.
			store.db
				.prepare(
					`UPDATE memory_items
					 SET actor_id = NULL,
					     origin_device_id = NULL,
					     metadata_json = ?
					 WHERE id = ?`,
				)
				.run(JSON.stringify({ actor_id: store.actorId, origin_device_id: store.deviceId }), memId);

			// Ownership check should succeed now that metadata_json is parsed.
			const result = store.moveMemoryProject(memId, "legacy-move");
			expect(result.project).toBe("legacy-move");
		});
	});

	describe("close", () => {
		it("closes the database connection", () => {
			const sessionId = insertTestSession(store.db);
			store.remember(sessionId, "discovery", "Title", "Body");
			store.close();

			// After close, operations should throw
			expect(() => store.get(1)).toThrow();
			// Prevent afterEach from double-closing
			store = undefined as unknown as MemoryStore;
		});
	});

	describe("Slice 1 durable raw-event contracts", () => {
		const repositoryIdentity = `repo-v1:sha256:${"a".repeat(64)}`;
		const testProviderManifest = (
			modelId: string,
			endpointUrl = "https://summary.stub.invalid/v1/chat/completions",
		) =>
			compileDefaultCapabilityManifest({
				version: 1,
				role: "summary",
				state: "enabled",
				wireProtocol: "openai_chat_completions_v1",
				modelId,
				modelRevision: "1",
				endpointUrl,
				credentialRef: { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" },
			});
		const providerManifest = testProviderManifest("store-test-summary");
		const localProviderManifest = testProviderManifest(
			"store-test-local-summary",
			"https://127.0.0.1:43123/v1/chat/completions",
		);
		const manifestFingerprint = providerManifest.configurationFingerprint;
		const providerFingerprint = providerManifest.summaryProvider.providerFingerprint;
		const providerBoundary = (durable: MemoryStore, jobId: number, manifest = providerManifest) =>
			compileProviderDestinationBoundary(manifest, {
				repositoryIdentity: durable.rawEventFlushJobRepositoryIdentity(jobId),
				tlsPeerVerified: true,
			});
		const claimFlushJob = (
			durable: MemoryStore,
			jobId: number,
			manifest = providerManifest,
			maxMemoryItemsPerDerivation?: number,
		) =>
			durable.claimRawEventFlushJob({
				jobId,
				manifestFingerprint: manifest.configurationFingerprint,
				providerFingerprint: manifest.summaryProvider.providerFingerprint,
				manifest,
				boundary: providerBoundary(durable, jobId, manifest),
				...(maxMemoryItemsPerDerivation === undefined ? {} : { maxMemoryItemsPerDerivation }),
			});
		const reopenCompletedFlushJob = (
			claim: NonNullable<ReturnType<MemoryStore["claimRawEventFlushJob"]>>,
			manifest = providerManifest,
		) => {
			store.db
				.prepare(
					`UPDATE raw_event_flush_batches
					 SET status = 'queued', completion_disposition = 'none', output_count = 0,
						observed_output_count = 0, egress_diagnostic_json = NULL
					 WHERE id = ? AND status = 'completed'`,
				)
				.run(claim.jobId);
			store.db
				.prepare(
					`UPDATE raw_event_sessions SET last_flushed_event_seq = ?
					 WHERE source = ? AND stream_id = ?`,
				)
				.run(claim.startEventSeq - 1, claim.source, claim.streamId);
			const next = claimFlushJob(store, claim.jobId, manifest);
			if (!next) throw new Error("expected reopened claim");
			return next;
		};

		const capture = (
			durable: MemoryStore,
			eventId: string,
			payload: Record<string, unknown>,
			overrides: Partial<Parameters<MemoryStore["recordRawEvent"]>[0]> = {},
		) =>
			durable.recordRawEvent({
				opencodeSessionId: "slice1",
				eventId,
				eventType: "user_prompt",
				payload,
				repositoryIdentity,
				captureManifestFingerprint: manifestFingerprint,
				sensitivity: "eligible",
				source: "codex",
				tsWallMs: Date.now() - 10_000,
				...overrides,
			});
		const insertRetryExhaustedJob = (
			streamId: string,
			attemptManifestFingerprint = manifestFingerprint,
			attemptProviderFingerprint = providerFingerprint,
		): number => {
			const now = "2026-08-31T00:00:00.000Z";
			capture(store, `${streamId}-0`, {}, { opencodeSessionId: streamId });
			return Number(
				store.db
					.prepare(
						`INSERT INTO raw_event_flush_batches(
							source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
							extractor_version, status, attempt_count, claim_generation,
							attempt_manifest_fingerprint, attempt_provider_fingerprint,
							created_at, updated_at
						 ) VALUES ('codex', ?, ?, 0, 0, 'raw_events_v1', 'retry_exhausted', 3, 0, ?, ?, ?, ?)`,
					)
					.run(streamId, streamId, attemptManifestFingerprint, attemptProviderFingerprint, now, now)
					.lastInsertRowid,
			);
		};
		const completeDerivedMemory = (input: {
			streamId: string;
			eventId: string;
			repositoryIdentity: string;
			sensitivity: "eligible" | "private";
			title: string;
			body: string;
		}) => {
			capture(
				store,
				input.eventId,
				{ text: input.body },
				{
					opencodeSessionId: input.streamId,
					repositoryIdentity: input.repositoryIdentity,
					sensitivity: input.sensitivity,
				},
			);
			const manifest = input.sensitivity === "eligible" ? providerManifest : localProviderManifest;
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId: input.streamId,
				manifestFingerprint: manifest.configurationFingerprint,
				providerFingerprint: manifest.summaryProvider.providerFingerprint,
			});
			const claim = claimFlushJob(store, admission.jobId as number, manifest);
			if (!claim) throw new Error("expected derivation claim");
			const sessionId = store.getOrCreateSessionForOpencodeSession({
				opencodeSessionId: input.streamId,
				source: "codex",
			});
			return store.completeRawEventFlushJobMemory(
				{ claim, sourceEventIds: [input.eventId], observedOutputCount: 1, diagnostic: {} },
				(_newMemoryIdFloor, derivation) => [
					derivation.remember({
						sourceCitations: [{ source: 0, start: null, end: null }],
						sessionId,
						kind: "discovery",
						title: input.title,
						bodyText: input.body,
					}),
				],
			);
		};

		it("persists repository-aware idempotency, monotonic quarantine, and pair-bound conflicts", () => {
			const durable = store;
			const first = capture(durable, "event-1", { text: "canonical" });
			expect(first).toMatchObject({ status: "accepted", normalAck: true });

			expect(
				capture(durable, "event-1", { text: "canonical" }, { sensitivity: "private" }),
			).toMatchObject({ status: "idempotent", normalAck: true });
			expect(
				capture(
					durable,
					"event-1",
					{ text: "canonical" },
					{
						sensitivity: "secret",
						captureState: "quarantined",
						safeErrorCode: "redaction_degraded",
					},
				),
			).toMatchObject({ status: "quarantined", normalAck: false });
			expect(
				store.db
					.prepare(
						"SELECT payload_json, sensitivity, capture_state FROM raw_events WHERE event_id = 'event-1'",
					)
					.get(),
			).toEqual({ payload_json: "{}", sensitivity: "secret", capture_state: "quarantined" });

			const conflict = capture(durable, "event-1", { text: "different" });
			const samePair = capture(durable, "event-1", { text: "different" });
			const otherPair = capture(durable, "event-1", { text: "third" });
			expect([conflict, samePair, otherPair]).toMatchObject([
				{ status: "identity_conflict", normalAck: false },
				{ status: "identity_conflict", normalAck: false },
				{ status: "identity_conflict", normalAck: false },
			]);
			expect(conflict.receiptId).toBe(samePair.receiptId);
			expect(otherPair.receiptId).not.toBe(conflict.receiptId);
			expect(
				store.db.prepare("SELECT COUNT(*) AS count FROM raw_event_identity_conflicts").get(),
			).toEqual({ count: 2 });

			capture(durable, "unknown", { text: "unknown" }, { repositoryIdentity: null });
			const nullCollision = capture(
				durable,
				"unknown",
				{ text: "unknown" },
				{
					repositoryIdentity: null,
				},
			);
			const nullReplay = capture(
				durable,
				"unknown",
				{ text: "unknown" },
				{
					repositoryIdentity: null,
				},
			);
			expect(nullCollision).toMatchObject({ status: "quarantined", normalAck: false });
			expect(nullReplay.receiptId).toBe(nullCollision.receiptId);
			expect(
				store.db
					.prepare(
						"SELECT payload_json, sensitivity, capture_state, safe_error_code FROM raw_event_quarantine WHERE event_id = 'unknown'",
					)
					.get(),
			).toEqual({
				payload_json: '{"text":"unknown"}',
				sensitivity: "secret",
				capture_state: "quarantined",
				safe_error_code: "repository_identity_unknown_collision",
			});
			expect(
				store.db.prepare("SELECT COUNT(*) AS count FROM raw_event_flush_batches").get(),
			).toEqual({ count: 0 });
		});

		it("scopes replay sensitivity cascades to the originating source and stream", () => {
			const eventId = "shared-event-id";
			const derive = (source: string, streamId: string, label: string) => {
				const payload = { text: label };
				capture(store, eventId, payload, { opencodeSessionId: streamId, source });
				const admission = store.admitRawEventFlushJob({
					source,
					streamId,
					manifestFingerprint,
					providerFingerprint,
				});
				const claim = claimFlushJob(store, admission.jobId as number);
				if (!claim) throw new Error("expected claim");
				const sessionId = store.getOrCreateSessionForOpencodeSession({
					opencodeSessionId: streamId,
					source,
				});
				const completed = store.completeRawEventFlushJobMemory(
					{ claim, sourceEventIds: [eventId], observedOutputCount: 1, diagnostic: {} },
					(_newMemoryIdFloor, derivation) => [
						derivation.remember({
							sourceCitations: [{ source: 0, start: null, end: null }],
							sessionId,
							kind: "discovery",
							title: label,
							bodyText: label,
						}),
					],
				);
				return { memoryId: completed.memoryIds[0] as number, payload };
			};

			const target = derive("codex", "target-stream", "target");
			const sameSource = derive("codex", "sibling-stream", "same source");
			const sameStream = derive("claude", "target-stream", "same stream");
			const ambiguousSessionId = insertTestSession(store.db);
			const ambiguousMemoryId = store.remember(
				ambiguousSessionId,
				"discovery",
				"ambiguous",
				"ambiguous",
			);
			store.db
				.prepare(
					`UPDATE memory_items
					 SET sensitivity = 'eligible', repository_identity = ?, source_event_ids_json = ?
					 WHERE id = ?`,
				)
				.run(repositoryIdentity, JSON.stringify([eventId]), ambiguousMemoryId);
			const sensitivities = () => {
				const sensitivity = store.db
					.prepare("SELECT sensitivity FROM memory_items WHERE id = ?")
					.pluck();
				return [target.memoryId, sameSource.memoryId, sameStream.memoryId, ambiguousMemoryId].map(
					(memoryId) => sensitivity.get(memoryId),
				);
			};

			expect(
				capture(store, eventId, target.payload, {
					opencodeSessionId: "target-stream",
					source: "codex",
					sensitivity: "private",
				}),
			).toMatchObject({ status: "idempotent", normalAck: true });
			expect(sensitivities()).toEqual(["private", "eligible", "eligible", "eligible"]);

			expect(
				capture(store, eventId, target.payload, {
					opencodeSessionId: "target-stream",
					source: "codex",
					sensitivity: "secret",
					captureState: "quarantined",
					safeErrorCode: "redaction_degraded",
				}),
			).toMatchObject({ status: "quarantined", normalAck: false });
			expect(sensitivities()).toEqual(["secret", "eligible", "eligible", "eligible"]);
		});

		it.each([
			repositoryIdentity,
			null,
		])("uses the repository identity index for %s capture lookup", (identity) => {
			const plan = store.db
				.prepare(
					`EXPLAIN QUERY PLAN
						 SELECT id FROM raw_events
						 WHERE COALESCE(repository_identity, 'repo-v1:unknown') = ?
						   AND source = ? AND stream_id = ? AND event_id = ?`,
				)
				.all(identity ?? "repo-v1:unknown", "codex", "slice1", "event-1") as Array<{
				detail: string;
			}>;
			expect(
				plan.some((row) => row.detail.includes("idx_raw_events_repository_source_stream_event_id")),
			).toBe(true);
		});

		it("normalizes malformed batch sensitivity to secret", () => {
			expect(
				store.recordRawEventsBatch(
					"malformed-sensitivity",
					[
						{
							event_id: "malformed-sensitivity",
							event_type: "user_prompt",
							payload: { text: "synthetic" },
							repository_identity: repositoryIdentity,
							sensitivity: "malformed",
						},
					],
					"codex",
				),
			).toMatchObject({ inserted: 1, skipped: 0 });
			expect(
				store.db
					.prepare("SELECT sensitivity FROM raw_events WHERE event_id = 'malformed-sensitivity'")
					.get(),
			).toEqual({ sensitivity: "secret" });

			expect(
				store.recordRawEventsBatch(
					"contradictory-quarantine",
					[
						{
							event_id: "contradictory-quarantine",
							event_type: "user_prompt",
							payload: { text: "must be scrubbed" },
							repository_identity: repositoryIdentity,
							sensitivity: "secret",
							capture_state: "quarantined",
							safe_error_code: "redaction_degraded",
							redaction_degraded: false,
						},
					],
					"codex",
				),
			).toMatchObject({
				inserted: 0,
				outcomes: [{ status: "quarantined", normalAck: false }],
			});
			expect(
				store.db
					.prepare(
						"SELECT payload_json FROM raw_events WHERE event_id = 'contradictory-quarantine'",
					)
					.get(),
			).toEqual({ payload_json: "{}" });
		});

		it("keeps multi-source memory sensitivity monotonic when one source strengthens", () => {
			capture(store, "multi-secret", { text: "secret source" }, { sensitivity: "secret" });
			capture(
				store,
				"multi-eligible",
				{ text: "eligible source" },
				{
					sensitivity: "eligible",
				},
			);
			const sessionId = insertTestSession(store.db);
			const memoryId = store.remember(sessionId, "discovery", "Multi-source", "Multi-source");
			store.db
				.prepare(
					`UPDATE memory_items
					 SET sensitivity = 'secret', repository_identity = ?, source_event_ids_json = ?
					 WHERE id = ?`,
				)
				.run(repositoryIdentity, '["multi-secret","multi-eligible"]', memoryId);

			expect(
				capture(
					store,
					"multi-eligible",
					{ text: "eligible source" },
					{
						sensitivity: "private",
					},
				),
			).toMatchObject({ status: "idempotent", normalAck: true });
			expect(
				store.db
					.prepare("SELECT sensitivity FROM raw_events WHERE event_id = ?")
					.get("multi-eligible"),
			).toEqual({ sensitivity: "private" });
			expect(
				store.db.prepare("SELECT sensitivity FROM memory_items WHERE id = ?").get(memoryId),
			).toEqual({ sensitivity: "secret" });
		});

		it("keeps same-title raw derivations separate across repositories", () => {
			const first = completeDerivedMemory({
				streamId: "derived-cross-repository",
				eventId: "derived-repository-a",
				repositoryIdentity,
				sensitivity: "eligible",
				title: "Shared derived title",
				body: "Shared derived body",
			});
			const second = completeDerivedMemory({
				streamId: "derived-cross-repository",
				eventId: "derived-repository-b",
				repositoryIdentity: `repo-v1:sha256:${"d".repeat(64)}`,
				sensitivity: "eligible",
				title: "Shared derived title",
				body: "Shared derived body",
			});

			expect(second.memoryIds[0]).not.toBe(first.memoryIds[0]);
			expect(
				store.db
					.prepare(
						"SELECT status, attempt_count FROM raw_event_flush_batches WHERE stream_id = ? ORDER BY id",
					)
					.all("derived-cross-repository"),
			).toEqual([
				{ status: "completed", attempt_count: 1 },
				{ status: "completed", attempt_count: 1 },
			]);
		});

		it("does not merge derived provenance into a legacy title-only dedup key", () => {
			const streamId = "derived-legacy-dedup";
			const title = "Legacy dedup title";
			const sessionId = store.getOrCreateSessionForOpencodeSession({
				opencodeSessionId: streamId,
				source: "codex",
			});
			const legacyMemoryId = store.remember(sessionId, "discovery", title, "Legacy body");
			store.db
				.prepare(
					`UPDATE memory_items SET repository_identity = ?, sensitivity = 'eligible',
						source_event_ids_json = '["legacy-event"]'
					 WHERE id = ?`,
				)
				.run(repositoryIdentity, legacyMemoryId);

			const derived = completeDerivedMemory({
				streamId,
				eventId: "derived-legacy-dedup-0",
				repositoryIdentity,
				sensitivity: "eligible",
				title,
				body: "Derived body",
			});

			expect(derived.memoryIds[0]).not.toBe(legacyMemoryId);
		});

		it("keeps same-title raw derivations separate across source events", () => {
			const first = completeDerivedMemory({
				streamId: "derived-stronger-sensitivity",
				eventId: "derived-eligible",
				repositoryIdentity,
				sensitivity: "eligible",
				title: "Sensitivity derived title",
				body: "Sensitivity derived body",
			});
			const second = completeDerivedMemory({
				streamId: "derived-stronger-sensitivity",
				eventId: "derived-private",
				repositoryIdentity,
				sensitivity: "private",
				title: "Sensitivity derived title",
				body: "Sensitivity derived body",
			});

			expect(second.memoryIds[0]).not.toBe(first.memoryIds[0]);
			expect(
				store.db
					.prepare(
						"SELECT id, sensitivity, source_event_ids_json FROM memory_items WHERE id IN (?, ?) ORDER BY id",
					)
					.all(first.memoryIds[0], second.memoryIds[0]),
			).toEqual([
				{
					id: first.memoryIds[0],
					sensitivity: "eligible",
					source_event_ids_json: '["derived-eligible"]',
				},
				{
					id: second.memoryIds[0],
					sensitivity: "private",
					source_event_ids_json: '["derived-private"]',
				},
			]);
			expect(
				capture(
					store,
					"derived-private",
					{ text: "Sensitivity derived body" },
					{
						opencodeSessionId: "derived-stronger-sensitivity",
						repositoryIdentity,
						sensitivity: "secret",
					},
				),
			).toMatchObject({ status: "idempotent", normalAck: true });
			expect(
				store.db
					.prepare("SELECT id, sensitivity FROM memory_items WHERE id IN (?, ?) ORDER BY id")
					.all(first.memoryIds[0], second.memoryIds[0]),
			).toEqual([
				{ id: first.memoryIds[0], sensitivity: "eligible" },
				{ id: second.memoryIds[0], sensitivity: "secret" },
			]);
			expect(
				store.db
					.prepare(
						"SELECT status, attempt_count FROM raw_event_flush_batches WHERE stream_id = ? ORDER BY id",
					)
					.all("derived-stronger-sensitivity"),
			).toEqual([
				{ status: "completed", attempt_count: 1 },
				{ status: "completed", attempt_count: 1 },
			]);
		});

		it("keeps same bare event IDs isolated across sibling streams", () => {
			const first = completeDerivedMemory({
				streamId: "derived-shared-id-a",
				eventId: "derived-shared-id",
				repositoryIdentity,
				sensitivity: "eligible",
				title: "Shared event ID title",
				body: "Shared event ID body",
			});
			const second = completeDerivedMemory({
				streamId: "derived-shared-id-b",
				eventId: "derived-shared-id",
				repositoryIdentity,
				sensitivity: "eligible",
				title: "Shared event ID title",
				body: "Shared event ID body",
			});
			expect(second.memoryIds[0]).not.toBe(first.memoryIds[0]);

			expect(
				capture(
					store,
					"derived-shared-id",
					{ text: "Shared event ID body" },
					{
						opencodeSessionId: "derived-shared-id-b",
						repositoryIdentity,
						sensitivity: "secret",
					},
				),
			).toMatchObject({ status: "idempotent", normalAck: true });
			expect(
				store.db
					.prepare("SELECT id, sensitivity FROM memory_items WHERE id IN (?, ?) ORDER BY id")
					.all(first.memoryIds[0], second.memoryIds[0]),
			).toEqual([
				{ id: first.memoryIds[0], sensitivity: "eligible" },
				{ id: second.memoryIds[0], sensitivity: "secret" },
			]);
		});

		it("rejects duplicate anchors in one response atomically", () => {
			const streamId = "derived-same-citation";
			const eventId = "derived-same-citation-0";
			capture(store, eventId, {}, { opencodeSessionId: streamId });
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			const claim = claimFlushJob(store, admission.jobId as number);
			if (!claim) throw new Error("expected same-citation claim");
			const sessionId = store.getOrCreateSessionForOpencodeSession({
				opencodeSessionId: streamId,
				source: "codex",
			});
			const before = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get();
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{ claim, sourceEventIds: [eventId], observedOutputCount: 2, diagnostic: {} },
					(_newMemoryIdFloor, derivation) => {
						const input = {
							sourceCitations: [{ source: 0, start: null, end: null }],
							sessionId,
							kind: "discovery",
							title: "Same citation title",
							bodyText: "Same citation body",
						};
						return [derivation.remember(input), derivation.remember(input)];
					},
				),
			).toThrow(/duplicate|anchor/i);
			expect(store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get()).toEqual(before);
			expect(store.rawEventFlushState(streamId, "codex")).toBe(-1);
		});

		it("rolls back caller-supplied completions that bypass the trusted derivation", () => {
			const streamId = "derived-citation-binding";
			capture(
				store,
				"derived-private-source",
				{ text: "private" },
				{
					opencodeSessionId: streamId,
					sensitivity: "private",
				},
			);
			capture(
				store,
				"derived-eligible-source",
				{ text: "eligible" },
				{
					opencodeSessionId: streamId,
					sensitivity: "eligible",
				},
			);
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			const claim = claimFlushJob(store, admission.jobId as number);
			if (!claim) throw new Error("expected citation-binding claim");
			const sessionId = store.getOrCreateSessionForOpencodeSession({
				opencodeSessionId: streamId,
				source: "codex",
			});
			const before = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get();

			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim,
						sourceEventIds: ["derived-private-source", "derived-eligible-source"],
						observedOutputCount: 1,
						diagnostic: {},
					},
					() => [
						{
							memoryId: store.remember(
								sessionId,
								"discovery",
								"Unbound completion",
								"Unbound completion",
							),
							disposition: "inserted" as const,
						},
					],
				),
			).toThrow(/binding|completion|output count/i);
			expect(store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get()).toEqual(before);
			expect(store.rawEventFlushState(streamId, "codex")).toBe(-1);
			expect(
				store.db
					.prepare("SELECT status FROM raw_event_flush_batches WHERE id = ?")
					.get(claim.jobId),
			).toEqual({ status: "processing" });
		});

		it.each([
			["repeated event IDs", ["shared-event-id", "shared-event-id"]],
			["distinct event IDs", ["repository-a-event", "repository-b-event"]],
		] as const)("splits repository changes with %s into sequential jobs", (_case, eventIds) => {
			const streamId = "repository-scoped-event-id";
			const otherRepository = `repo-v1:sha256:${"d".repeat(64)}`;
			const cases = [
				[repositoryIdentity, "private"],
				[otherRepository, "eligible"],
			] as const;
			for (const [index, [repository, sensitivity]] of cases.entries()) {
				capture(
					store,
					eventIds[index] as string,
					{ repository },
					{
						opencodeSessionId: streamId,
						repositoryIdentity: repository,
						sensitivity,
					},
				);
			}
			const sessionId = store.getOrCreateSessionForOpencodeSession({
				opencodeSessionId: streamId,
				source: "codex",
			});

			for (const [sequence, [repository, sensitivity]] of cases.entries()) {
				const manifest = sensitivity === "eligible" ? providerManifest : localProviderManifest;
				const admission = store.admitRawEventFlushJob({
					source: "codex",
					streamId,
					manifestFingerprint: manifest.configurationFingerprint,
					providerFingerprint: manifest.summaryProvider.providerFingerprint,
				});
				expect(admission).toMatchObject({
					status: "admitted",
					startEventSeq: sequence,
					endEventSeq: sequence,
				});
				const claim = claimFlushJob(store, admission.jobId as number, manifest);
				if (!claim) throw new Error("expected repository-scoped claim");
				const eventId = eventIds[sequence] as string;
				const completed = store.completeRawEventFlushJobMemory(
					{ claim, sourceEventIds: [eventId], observedOutputCount: 1, diagnostic: {} },
					(_newMemoryIdFloor, derivation) => [
						derivation.remember({
							sourceCitations: [{ source: 0, start: null, end: null }],
							sessionId,
							kind: "discovery",
							title: repository,
							bodyText: repository,
						}),
					],
				);
				expect(
					store.db
						.prepare("SELECT repository_identity, sensitivity FROM memory_items WHERE id = ?")
						.get(completed.memoryIds[0]),
				).toEqual({
					repository_identity: repository,
					sensitivity,
				});
				expect(store.rawEventFlushState(streamId, "codex")).toBe(sequence);
			}

			expect(
				store.db
					.prepare(
						"SELECT status, attempt_count FROM raw_event_flush_batches WHERE stream_id = ? ORDER BY start_event_seq",
					)
					.all(streamId),
			).toEqual([
				{ status: "completed", attempt_count: 1 },
				{ status: "completed", attempt_count: 1 },
			]);
		});

		it("bounds admission and claims, exhausts attempts, and makes grant consumption atomic", () => {
			const durable = store;
			const seedStream = (
				streamId: string,
				count: number,
				sensitivity: Parameters<MemoryStore["recordRawEvent"]>[0]["sensitivity"] = "secret",
			) => {
				for (let index = 0; index < count; index++) {
					capture(
						durable,
						`${streamId}-${index}`,
						{ index },
						{
							opencodeSessionId: streamId,
							sensitivity,
						},
					);
				}
			};
			const admit = (streamId: string) =>
				durable.admitRawEventFlushJob({
					source: "codex",
					streamId,
					manifestFingerprint,
					providerFingerprint,
				});

			seedStream("jobs", 101);
			const first = admit("jobs");
			expect(first).toMatchObject({
				status: "admitted",
				startEventSeq: 0,
				endEventSeq: 99,
			});
			expect(admit("jobs")).toMatchObject({ status: "existing", jobId: first.jobId });

			seedStream("gap", 3);
			store.db.prepare("DELETE FROM raw_events WHERE stream_id = 'gap' AND event_seq = 1").run();
			expect(admit("gap")).toMatchObject({ status: "source_gap" });
			seedStream("fully-pruned", 1);
			store.db.prepare("DELETE FROM raw_events WHERE stream_id = 'fully-pruned'").run();
			expect(admit("fully-pruned")).toMatchObject({ status: "source_gap" });

			const admittedIds = [first.jobId as number];
			for (let index = 0; index < 24; index++) {
				const streamId = `capacity-${index}`;
				seedStream(streamId, 1);
				admittedIds.push(admit(streamId).jobId as number);
			}
			store.db
				.prepare("UPDATE raw_event_flush_batches SET status = 'retry_exhausted' WHERE id = ?")
				.run(admittedIds[23]);
			seedStream("over-capacity", 1);
			expect(admit("over-capacity")).toMatchObject({ status: "capacity" });
			expect(
				store.db
					.prepare(
						"SELECT COUNT(*) AS count FROM raw_event_flush_batches WHERE status IN ('queued','processing','failed','retry_exhausted')",
					)
					.get(),
			).toEqual({ count: 25 });

			const claim = (jobId: number) => claimFlushJob(durable, jobId);
			let firstClaim = claim(admittedIds[0] as number);
			const secondClaim = claim(admittedIds[1] as number);
			expect(firstClaim).toMatchObject({ attemptCount: 1, claimGeneration: 1 });
			expect(secondClaim).toMatchObject({ attemptCount: 1, claimGeneration: 1 });
			expect(claim(admittedIds[2] as number)).toBeNull();
			if (!firstClaim) throw new Error("expected first claim");

			for (const expectedStatus of ["failed", "failed", "retry_exhausted"] as const) {
				expect(
					durable.failRawEventFlushJob({
						jobId: firstClaim.jobId,
						claimGeneration: firstClaim.claimGeneration,
						attemptFingerprint: firstClaim.attemptFingerprint,
						safeErrorCode: "provider_unavailable",
					}),
				).toEqual({ status: expectedStatus });
				if (expectedStatus === "retry_exhausted") break;
				expect(durable.requeueFailedRawEventFlushJob(firstClaim.jobId)).toBe(true);
				const next = claim(firstClaim.jobId);
				if (!next) throw new Error("expected automatic retry claim");
				firstClaim = next;
			}

			const signal = durable.applyResumeSignal({
				signalId: "doctor-signal-1",
				producerReceiptId: "doctor-receipt-1",
				targetJobId: firstClaim.jobId,
				sequence: 1,
				kind: "user_confirmed_doctor_retry",
				targetRole: "summary",
				providerFingerprint,
				manifestFingerprint,
			});
			expect(signal).toMatchObject({ disposition: "accepted", grantState: "pending" });
			expect(
				durable.applyResumeSignal({
					signalId: "doctor-signal-1",
					producerReceiptId: "doctor-receipt-1",
					targetJobId: firstClaim.jobId,
					sequence: 1,
					kind: "user_confirmed_doctor_retry",
					targetRole: "summary",
					providerFingerprint,
					manifestFingerprint,
				}),
			).toMatchObject({ disposition: "duplicate", grantState: "pending" });
			expect(
				store.db
					.prepare(
						"SELECT last_resume_signal_disposition FROM raw_event_flush_batches WHERE id = ?",
					)
					.get(firstClaim.jobId),
			).toEqual({ last_resume_signal_disposition: "duplicate" });
			const resumed = claim(firstClaim.jobId);
			expect(resumed).toMatchObject({ attemptCount: 4, claimGeneration: 4 });
			if (!resumed) throw new Error("expected resumed claim");
			expect(() =>
				durable.completeRawEventFlushJobPrivacySkip({
					claim: firstClaim,
					sourceEventIds: Array.from({ length: 100 }, (_, index) => `jobs-${index}`),
					projection: {
						eligibleSourceEventIds: [],
						omittedSourceEventIds: Array.from({ length: 100 }, (_, index) => `jobs-${index}`),
					},
					diagnostic: {},
				}),
			).toThrow(/stale/i);
			expect(
				durable.completeRawEventFlushJobPrivacySkip({
					claim: resumed,
					sourceEventIds: Array.from({ length: 100 }, (_, index) => `jobs-${index}`),
					projection: {
						eligibleSourceEventIds: [],
						omittedSourceEventIds: Array.from({ length: 100 }, (_, index) => `jobs-${index}`),
					},
					diagnostic: {
						version: 1,
						action: "skipped",
						reason: "all_restricted",
						nextAction: "none",
					},
				}),
			).toEqual({ frontierChanged: true });
			expect(
				store.db
					.prepare(
						"SELECT status, attempt_count, resume_grant_state FROM raw_event_flush_batches WHERE id = ?",
					)
					.get(resumed.jobId),
			).toEqual({ status: "completed", attempt_count: 4, resume_grant_state: "consumed" });
			expect(store.rawEventFlushState("jobs", "codex")).toBe(99);
		});

		it("refuses a direct claim when the admitted source range is no longer exact", () => {
			const streamId = "direct-claim-source-gap";
			for (let index = 0; index < 3; index++) {
				capture(store, `${streamId}-${index}`, { index }, { opencodeSessionId: streamId });
			}
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			store.db
				.prepare("DELETE FROM raw_events WHERE stream_id = ? AND event_seq = 1")
				.run(streamId);

			expect(claimFlushJob(store, admission.jobId as number)).toBeNull();
			expect(
				store.db
					.prepare("SELECT status, attempt_count FROM raw_event_flush_batches WHERE id = ?")
					.get(admission.jobId),
			).toEqual({ status: "queued", attempt_count: 0 });
		});

		it("rediscovers granted complete-range legacy work and replays its consumed doctor receipt", () => {
			const streamId = "legacy-recovery";
			const eventId = "legacy-recovery-0";
			capture(
				store,
				eventId,
				{},
				{
					opencodeSessionId: streamId,
					redactionDegraded: true,
					captureState: "quarantined",
					safeErrorCode: "redaction_degraded",
				},
			);
			const admitted = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			if (!admitted.jobId) throw new Error("expected admitted legacy fixture");
			store.db
				.prepare(
					`UPDATE raw_event_sessions SET last_flushed_event_seq = 0
					 WHERE source = 'codex' AND stream_id = ?`,
				)
				.run(streamId);
			store.db
				.prepare(
					`UPDATE raw_event_flush_batches
					 SET status = 'retry_exhausted', attempt_count = 3,
						admission_manifest_fingerprint = NULL, admission_provider_fingerprint = NULL,
						attempt_manifest_fingerprint = NULL, attempt_provider_fingerprint = NULL,
						attempt_fingerprint = NULL,
						legacy_recovery_state = 'complete_range', frontier_already_advanced = 1
					 WHERE id = ?`,
				)
				.run(admitted.jobId);
			const confirmation = {
				jobId: admitted.jobId,
				producerReceiptId: "legacy-recovery-receipt",
				expectedRole: "summary" as const,
				expectedProviderFingerprint: null,
				expectedManifestFingerprint: null,
				expectedAttemptCount: 3,
				expectedClaimGeneration: 0,
				targetProviderFingerprint: providerFingerprint,
				targetManifestFingerprint: manifestFingerprint,
			};
			const accepted = store.confirmDoctorRetry(confirmation);
			expect(accepted).toMatchObject({ disposition: "accepted", grantState: "pending" });
			expect(
				store.db
					.prepare(
						`SELECT admission_manifest_fingerprint, admission_provider_fingerprint,
							attempt_manifest_fingerprint, attempt_provider_fingerprint,
							attempt_fingerprint, resume_grant_state
						 FROM raw_event_flush_batches WHERE id = ?`,
					)
					.get(admitted.jobId),
			).toEqual({
				admission_manifest_fingerprint: null,
				admission_provider_fingerprint: null,
				attempt_manifest_fingerprint: null,
				attempt_provider_fingerprint: null,
				attempt_fingerprint: null,
				resume_grant_state: "pending",
			});
			expect(store.rawEventSessionsWithPendingQueue()).toContainEqual({
				source: "codex",
				streamId,
			});
			expect(
				store.admitRawEventFlushJob({
					source: "codex",
					streamId,
					manifestFingerprint,
					providerFingerprint,
				}),
			).toMatchObject({ status: "existing", jobId: admitted.jobId });
			const claim = claimFlushJob(store, admitted.jobId);
			if (!claim) throw new Error("expected legacy recovery claim");
			expect(
				store.confirmDoctorRetry({
					...confirmation,
					targetProviderFingerprint: null,
					targetManifestFingerprint: null,
				}),
			).toEqual({
				...accepted,
				disposition: "duplicate",
				grantState: "consumed",
			});
			expect(
				store.db
					.prepare(
						`SELECT
							(SELECT COUNT(*) FROM processing_resume_signals WHERE job_id = ?) AS signal_count,
							(SELECT COUNT(*) FROM processing_resume_producer_receipts WHERE receipt_id = ?) AS receipt_count,
							(SELECT last_resume_signal_disposition FROM raw_event_flush_batches WHERE id = ?) AS last_disposition`,
					)
					.get(admitted.jobId, confirmation.producerReceiptId, admitted.jobId),
			).toEqual({ signal_count: 1, receipt_count: 1, last_disposition: "duplicate" });
			expect(
				store.db
					.prepare(
						`SELECT admission_manifest_fingerprint, admission_provider_fingerprint,
							attempt_manifest_fingerprint, attempt_provider_fingerprint, attempt_fingerprint
						 FROM raw_event_flush_batches WHERE id = ?`,
					)
					.get(admitted.jobId),
			).toEqual({
				admission_manifest_fingerprint: null,
				admission_provider_fingerprint: null,
				attempt_manifest_fingerprint: manifestFingerprint,
				attempt_provider_fingerprint: providerFingerprint,
				attempt_fingerprint: claim.attemptFingerprint,
			});
			expect(
				store.completeRawEventFlushJobPrivacySkip({
					claim,
					sourceEventIds: [eventId],
					projection: { eligibleSourceEventIds: [], omittedSourceEventIds: [eventId] },
					diagnostic: {},
				}),
			).toEqual({ frontierChanged: false });
			expect(store.rawEventFlushState(streamId, "codex")).toBe(0);
		});

		it("binds the derivation limit to the validated active manifest", () => {
			const base = compileDefaultCapabilityManifest({
				version: 1,
				role: "summary",
				state: "enabled",
				wireProtocol: "openai_chat_completions_v1",
				modelId: "store-limit-model",
				modelRevision: "1",
				endpointUrl: "https://summary.stub.invalid/v1/chat/completions",
				credentialRef: { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" },
			});
			capture(
				store,
				"base-limit-0",
				{},
				{
					opencodeSessionId: "base-limit",
					captureManifestFingerprint: base.configurationFingerprint,
				},
			);
			const baseAdmission = store.admitRawEventFlushJob({
				source: "codex",
				streamId: "base-limit",
				manifestFingerprint: base.configurationFingerprint,
				providerFingerprint: base.summaryProvider.providerFingerprint,
			});
			expect(() =>
				store.claimRawEventFlushJob({
					jobId: baseAdmission.jobId as number,
					manifestFingerprint: base.configurationFingerprint,
					providerFingerprint: base.summaryProvider.providerFingerprint,
					maxMemoryItemsPerDerivation: 17,
					manifest: base,
					boundary: providerBoundary(store, baseAdmission.jobId as number, base),
				}),
			).toThrow(/maxMemoryItemsPerDerivation/i);
			let baseClaim = claimFlushJob(store, baseAdmission.jobId as number, base);
			if (!baseClaim) throw new Error("expected base claim");
			for (let attempt = 1; attempt <= 3; attempt++) {
				expect(
					store.failRawEventFlushJob({
						jobId: baseClaim.jobId,
						claimGeneration: baseClaim.claimGeneration,
						attemptFingerprint: baseClaim.attemptFingerprint,
						safeErrorCode: "output_invalid",
					}),
				).toEqual({ status: attempt === 3 ? "retry_exhausted" : "failed" });
				if (attempt < 3) {
					expect(store.requeueFailedRawEventFlushJob(baseClaim.jobId)).toBe(true);
					const next = claimFlushJob(store, baseClaim.jobId, base);
					if (!next) throw new Error("expected next base claim");
					baseClaim = next;
				}
			}

			const successor = compileCapabilityManifest({
				manifestVersion: base.manifestVersion,
				manifestId: base.manifestId,
				baseConfigurationFingerprint: base.configurationFingerprint,
				destinationPolicyMap: base.destinationPolicyMap,
				resourceProfile: {
					...base.resourceProfile,
					version: 2,
					maxMemoryItemsPerDerivation: 17,
				},
				summaryProvider: {
					version: base.summaryProvider.version,
					role: base.summaryProvider.role,
					state: base.summaryProvider.state,
					wireProtocol: base.summaryProvider.wireProtocol,
					modelId: base.summaryProvider.modelId,
					modelRevision: base.summaryProvider.modelRevision,
					endpointUrl: base.summaryProvider.endpointUrl,
					credentialRef: base.summaryProvider.credentialRef,
				},
				embeddingProvider: base.embeddingProvider,
				legacyDispositions: base.legacyDispositions,
			});
			capture(
				store,
				"successor-limit-0",
				{},
				{
					opencodeSessionId: "successor-limit",
					captureManifestFingerprint: successor.configurationFingerprint,
				},
			);
			const successorAdmission = store.admitRawEventFlushJob({
				source: "codex",
				streamId: "successor-limit",
				manifestFingerprint: successor.configurationFingerprint,
				providerFingerprint: successor.summaryProvider.providerFingerprint,
			});
			expect(() =>
				store.claimRawEventFlushJob({
					jobId: successorAdmission.jobId as number,
					manifestFingerprint: successor.configurationFingerprint,
					providerFingerprint: successor.summaryProvider.providerFingerprint,
					maxMemoryItemsPerDerivation: 17,
					manifest: successor,
					boundary: providerBoundary(store, successorAdmission.jobId as number, successor),
				}),
			).toThrow(/recovery successor/i);
			expect(
				store.applyResumeSignal({
					signalId: "doctor-limit-signal-1",
					producerReceiptId: "doctor-limit-receipt-1",
					targetJobId: baseClaim.jobId,
					sequence: 1,
					kind: "user_confirmed_doctor_retry",
					targetRole: "summary",
					providerFingerprint: base.summaryProvider.providerFingerprint,
					manifestFingerprint: base.configurationFingerprint,
				}),
			).toMatchObject({ disposition: "accepted", grantState: "pending" });
			expect(
				store.applyResumeSignal({
					signalId: "doctor-limit-invalid-signal-2",
					producerReceiptId: "doctor-limit-invalid-receipt-2",
					targetJobId: baseClaim.jobId,
					sequence: 2,
					kind: "user_confirmed_doctor_retry",
					targetRole: "summary",
					providerFingerprint: `sha256:${"d".repeat(64)}`,
					manifestFingerprint: base.configurationFingerprint,
				}),
			).toMatchObject({ disposition: "unrelated_component", grantState: "pending" });
			expect(() => claimFlushJob(store, baseClaim.jobId, successor, 17)).toThrow(
				/resume grant target/i,
			);
			const doctorClaim = claimFlushJob(store, baseClaim.jobId, base);
			if (!doctorClaim) throw new Error("expected doctor claim");
			expect(
				store.failRawEventFlushJob({
					jobId: doctorClaim.jobId,
					claimGeneration: doctorClaim.claimGeneration,
					attemptFingerprint: doctorClaim.attemptFingerprint,
					safeErrorCode: "output_invalid",
				}),
			).toEqual({ status: "retry_exhausted" });
			expect(
				store.applyResumeSignal({
					signalId: "output-limit-signal-2",
					producerReceiptId: "output-limit-receipt-2",
					targetJobId: baseClaim.jobId,
					sequence: 2,
					kind: "validated_configuration_activation",
					targetRole: "summary",
					providerFingerprint: successor.summaryProvider.providerFingerprint,
					manifestFingerprint: successor.configurationFingerprint,
				}),
			).toMatchObject({ disposition: "accepted", grantState: "pending" });
			const successorClaim = claimFlushJob(store, baseClaim.jobId, successor, 17);
			expect(successorClaim).toMatchObject({
				maxMemoryItemsPerDerivation: 17,
				usedResumeGrant: true,
			});
			if (!successorClaim) throw new Error("expected successor claim");
			expect(
				store.failRawEventFlushJob({
					jobId: successorClaim.jobId,
					claimGeneration: successorClaim.claimGeneration,
					attemptFingerprint: successorClaim.attemptFingerprint,
					safeErrorCode: "output_invalid",
				}),
			).toEqual({ status: "retry_exhausted" });

			const chainedSuccessor = compileCapabilityManifest({
				manifestVersion: base.manifestVersion,
				manifestId: base.manifestId,
				baseConfigurationFingerprint: successor.configurationFingerprint,
				destinationPolicyMap: base.destinationPolicyMap,
				resourceProfile: successor.resourceProfile,
				summaryProvider: {
					version: base.summaryProvider.version,
					role: base.summaryProvider.role,
					state: base.summaryProvider.state,
					wireProtocol: base.summaryProvider.wireProtocol,
					modelId: base.summaryProvider.modelId,
					modelRevision: base.summaryProvider.modelRevision,
					endpointUrl: base.summaryProvider.endpointUrl,
					credentialRef: base.summaryProvider.credentialRef,
				},
				embeddingProvider: base.embeddingProvider,
				legacyDispositions: base.legacyDispositions,
			});
			expect(
				store.applyResumeSignal({
					signalId: "output-limit-signal-3",
					producerReceiptId: "output-limit-receipt-3",
					targetJobId: successorClaim.jobId,
					sequence: 3,
					kind: "validated_configuration_activation",
					targetRole: "summary",
					providerFingerprint: chainedSuccessor.summaryProvider.providerFingerprint,
					manifestFingerprint: chainedSuccessor.configurationFingerprint,
				}),
			).toMatchObject({ disposition: "accepted", grantState: "pending" });
			expect(() => claimFlushJob(store, successorClaim.jobId, chainedSuccessor, 17)).toThrow(
				/recovery successor/i,
			);
		});

		it("validates cited sources and stamps per-memory provenance atomically", () => {
			const citationManifest = localProviderManifest;
			for (const [eventId, sensitivity] of [
				["cited-0", "eligible"],
				["cited-1", "private"],
			] as const) {
				store.recordRawEvent({
					opencodeSessionId: "cited",
					source: "codex",
					eventId,
					eventType: "user_prompt",
					payload: { eventId },
					repositoryIdentity,
					captureManifestFingerprint: citationManifest.configurationFingerprint,
					sensitivity,
				});
			}
			expect(() =>
				store.admitRawEventFlushJob({
					source: "codex",
					streamId: "cited",
					manifestFingerprint: null as never,
					providerFingerprint: citationManifest.summaryProvider.providerFingerprint,
				}),
			).toThrow(/required/i);
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId: "cited",
				manifestFingerprint: citationManifest.configurationFingerprint,
				providerFingerprint: citationManifest.summaryProvider.providerFingerprint,
			});
			expect(() =>
				store.claimRawEventFlushJob({
					jobId: admission.jobId as number,
					manifestFingerprint: citationManifest.configurationFingerprint,
					providerFingerprint: null as never,
				}),
			).toThrow(/required/i);
			const claim = claimFlushJob(store, admission.jobId as number, citationManifest);
			if (!claim) throw new Error("expected cited claim");
			const sessionId = insertTestSession(store.db);
			const countMemories = () =>
				(store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get() as { count: number })
					.count;
			const before = countMemories();
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim: { ...claim, maxMemoryItemsPerDerivation: Number.NaN },
						sourceEventIds: ["cited-0", "cited-1"],
						observedOutputCount: 0,
						diagnostic: {},
					},
					() => [],
				),
			).toThrow(/output limit/i);
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim: { ...claim, maxMemoryItemsPerDerivation: 17 },
						sourceEventIds: ["cited-0", "cited-1"],
						observedOutputCount: 0,
						diagnostic: {},
					},
					() => [],
				),
			).toThrow(/stale|binding/i);
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim: { ...claim, manifestFingerprint: `sha256:${"c".repeat(64)}` },
						sourceEventIds: ["cited-0", "cited-1"],
						observedOutputCount: 0,
						diagnostic: {},
					},
					() => [],
				),
			).toThrow(/stale|binding/i);
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim,
						sourceEventIds: ["cited-0", "cited-1"],
						observedOutputCount: 1,
						diagnostic: {},
					},
					(_newMemoryIdFloor, derivation) => [
						derivation.remember({
							sourceCitations: [],
							sessionId,
							kind: "discovery",
							title: "invalid",
							bodyText: "invalid",
						}),
					],
				),
			).toThrow(/citation/i);
			expect(countMemories()).toBe(before);
			expect(store.rawEventFlushState("cited", "codex")).toBe(-1);
			const unrelatedMemoryId = store.remember(sessionId, "discovery", "unrelated", "unrelated");
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim,
						sourceEventIds: ["cited-0", "cited-1"],
						observedOutputCount: 1,
						diagnostic: {},
					},
					() => [
						{
							memoryId: unrelatedMemoryId,
							disposition: "inserted",
						},
					],
				),
			).toThrow(/memory completion|output count/i);
			expect(
				store.db
					.prepare(
						"SELECT repository_identity, source_event_ids_json, manifest_fingerprint FROM memory_items WHERE id = ?",
					)
					.get(unrelatedMemoryId),
			).toEqual({
				repository_identity: null,
				source_event_ids_json: null,
				manifest_fingerprint: null,
			});
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim,
						sourceEventIds: ["cited-0", "cited-1"],
						observedOutputCount: 1,
						diagnostic: {},
					},
					() => [
						{
							memoryId: unrelatedMemoryId,
							disposition: "unexpected",
						} as never,
					],
				),
			).toThrow(/memory completion|output count/i);

			const completed = store.completeRawEventFlushJobMemory(
				{
					claim,
					sourceEventIds: ["cited-0", "cited-1"],
					observedOutputCount: 1,
					diagnostic: {},
				},
				(_newMemoryIdFloor, derivation) => [
					derivation.remember({
						sourceCitations: [{ source: 1, start: null, end: null }],
						sessionId,
						kind: "discovery",
						title: "valid",
						bodyText: "valid",
					}),
				],
			);
			expect(completed).toMatchObject({ frontierChanged: true, memoryIds: [expect.any(Number)] });
			expect(
				store.db
					.prepare(
						`SELECT sensitivity, repository_identity, source_event_ids_json,
							manifest_fingerprint, provider_fingerprint, attempt_fingerprint
						 FROM memory_items WHERE id = ?`,
					)
					.get(completed.memoryIds[0]),
			).toEqual({
				sensitivity: "private",
				repository_identity: repositoryIdentity,
				source_event_ids_json: '["cited-1"]',
				manifest_fingerprint: citationManifest.configurationFingerprint,
				provider_fingerprint: citationManifest.summaryProvider.providerFingerprint,
				attempt_fingerprint: claim.attemptFingerprint,
			});

			store.recordRawEvent({
				opencodeSessionId: "deduplicated-output",
				source: "codex",
				eventId: "deduplicated-0",
				eventType: "user_prompt",
				payload: { text: "duplicate" },
				repositoryIdentity,
				captureManifestFingerprint: manifestFingerprint,
				sensitivity: "eligible",
			});
			const dedupAdmission = store.admitRawEventFlushJob({
				source: "codex",
				streamId: "deduplicated-output",
				manifestFingerprint,
				providerFingerprint,
			});
			const dedupClaim = claimFlushJob(store, dedupAdmission.jobId as number);
			if (!dedupClaim) throw new Error("expected dedup claim");
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim: dedupClaim,
						sourceEventIds: ["deduplicated-0"],
						observedOutputCount: 1,
						diagnostic: {},
					},
					() => [
						{
							memoryId: store.remember(sessionId, "discovery", "valid", "valid"),
							disposition: "deduplicated",
						},
					],
				),
			).toThrow(/citation binding|output count/i);
			expect(
				store.db
					.prepare(
						"SELECT source_event_ids_json, attempt_fingerprint FROM memory_items WHERE id = ?",
					)
					.get(completed.memoryIds[0]),
			).toEqual({
				source_event_ids_json: '["cited-1"]',
				attempt_fingerprint: claim.attemptFingerprint,
			});

			const otherRepository = `repo-v1:sha256:${"d".repeat(64)}`;
			for (const eventId of ["mixed-0", "mixed-1"]) {
				store.recordRawEvent({
					opencodeSessionId: "mixed-repositories",
					source: "codex",
					eventId,
					eventType: "user_prompt",
					payload: { eventId },
					repositoryIdentity,
					captureManifestFingerprint: manifestFingerprint,
					sensitivity: "eligible",
				});
			}
			const mixedAdmission = store.admitRawEventFlushJob({
				source: "codex",
				streamId: "mixed-repositories",
				manifestFingerprint,
				providerFingerprint,
			});
			const mixedClaim = claimFlushJob(store, mixedAdmission.jobId as number);
			if (!mixedClaim) throw new Error("expected mixed claim");
			store.db
				.prepare("UPDATE raw_events SET repository_identity = ? WHERE event_id = 'mixed-1'")
				.run(otherRepository);
			const beforeMixed = countMemories();
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim: mixedClaim,
						sourceEventIds: ["mixed-0", "mixed-1"],
						observedOutputCount: 1,
						diagnostic: {},
					},
					() => [
						{
							memoryId: store.remember(sessionId, "discovery", "mixed", "mixed"),
							disposition: "inserted",
						},
					],
				),
			).toThrow(/one repository|source set/i);
			expect(countMemories()).toBe(beforeMixed);
			expect(store.rawEventFlushState("mixed-repositories", "codex")).toBe(-1);
		});

		it("binds a compiler-created boundary and projects only eligible sources in order", () => {
			const streamId = "claim-bound-projection";
			for (const [eventId, sensitivity] of [
				["projected-eligible-0", "eligible"],
				["projected-private-1", "private"],
				["projected-eligible-2", "eligible"],
				["projected-local-only-3", "local_only"],
				["projected-secret-4", "secret"],
			] as const) {
				capture(store, eventId, { eventId }, { opencodeSessionId: streamId, sensitivity });
			}
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			const jobId = admission.jobId as number;
			const validBoundary = providerBoundary(store, jobId);
			expect(() =>
				store.claimRawEventFlushJob({
					jobId,
					manifestFingerprint,
					providerFingerprint,
					manifest: providerManifest,
					boundary: { ...validBoundary },
				}),
			).toThrow(/compiler|boundary/i);
			expect(
				store.db
					.prepare("SELECT status, attempt_count FROM raw_event_flush_batches WHERE id = ?")
					.get(jobId),
			).toEqual({ status: "queued", attempt_count: 0 });

			const wrongManifest = testProviderManifest("wrong-boundary-provider");
			expect(() =>
				store.claimRawEventFlushJob({
					jobId,
					manifestFingerprint,
					providerFingerprint,
					manifest: providerManifest,
					boundary: providerBoundary(store, jobId, wrongManifest),
				}),
			).toThrow(/boundary/i);

			const claim = claimFlushJob(store, jobId);
			if (!claim) throw new Error("expected projected claim");
			expect(store.rawEventFlushClaimSourceEventIds(claim)).toEqual([
				"projected-eligible-0",
				"projected-private-1",
				"projected-eligible-2",
				"projected-local-only-3",
				"projected-secret-4",
			]);
			expect(store.rawEventFlushClaimProjectedSourceSet(claim).sources).toMatchObject([
				{ ordinal: 0, eventId: "projected-eligible-0", sensitivity: "eligible" },
				{ ordinal: 1, eventId: "projected-eligible-2", sensitivity: "eligible" },
			]);
			expect(store.loadRawEventFlushJobEvents(claim).map((event) => event.event_id)).toEqual([
				"projected-eligible-0",
				"projected-eligible-2",
			]);
			expect(() => store.rawEventFlushClaimProjectedSourceSet({ ...claim })).toThrow(/stale/i);
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim: { ...claim },
						sourceEventIds: store.rawEventFlushClaimSourceEventIds(claim),
						observedOutputCount: 0,
						diagnostic: {},
					},
					() => [],
				),
			).toThrow(/stale/i);

			const restrictedStream = "claim-bound-all-restricted";
			for (const [eventId, sensitivity] of [
				["restricted-private", "private"],
				["restricted-local", "local_only"],
				["restricted-secret", "secret"],
			] as const) {
				capture(store, eventId, { eventId }, { opencodeSessionId: restrictedStream, sensitivity });
			}
			const restrictedAdmission = store.admitRawEventFlushJob({
				source: "codex",
				streamId: restrictedStream,
				manifestFingerprint,
				providerFingerprint,
			});
			const restrictedClaim = claimFlushJob(store, restrictedAdmission.jobId as number);
			if (!restrictedClaim) throw new Error("expected all-restricted claim");
			const restrictedIds = store.rawEventFlushClaimSourceEventIds(restrictedClaim);
			expect(store.rawEventFlushClaimProjectedSourceSet(restrictedClaim).sources).toEqual([]);
			expect(
				store.completeRawEventFlushJobPrivacySkip({
					claim: restrictedClaim,
					sourceEventIds: restrictedIds,
					projection: { eligibleSourceEventIds: [], omittedSourceEventIds: restrictedIds },
					diagnostic: { reason: "all_restricted" },
				}),
			).toEqual({ frontierChanged: true });
		});

		it("normalizes whole-event and UTF-8 citations while preserving ID order", () => {
			const streamId = "claim-bound-span-normalization";
			capture(store, "z-source", { text: "alpha" }, { opencodeSessionId: streamId });
			capture(store, "a-source", { text: "猫" }, { opencodeSessionId: streamId });
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			const claim = claimFlushJob(store, admission.jobId as number);
			if (!claim) throw new Error("expected span claim");
			expect(store.rawEventFlushClaimProjectedSourceSet(claim).sources).toMatchObject([
				{ ordinal: 0, eventId: "z-source", redactedPayload: '{"text":"alpha"}' },
				{ ordinal: 1, eventId: "a-source", redactedPayload: '{"text":"猫"}' },
			]);
			const sessionId = insertTestSession(store.db);
			const completed = store.completeRawEventFlushJobMemory(
				{
					claim,
					sourceEventIds: ["z-source", "a-source"],
					observedOutputCount: 1,
					diagnostic: {},
				},
				(_newMemoryIdFloor, derivation) => [
					derivation.remember({
						sourceCitations: [
							{ source: 0, start: null, end: null },
							{ source: 1, start: 9, end: 12 },
						],
						sessionId,
						kind: "discovery",
						title: "Ordered UTF-8 anchors",
						bodyText: "Body",
					}),
				],
			);
			expect(
				store.db
					.prepare("SELECT source_event_ids_json, source_spans_json FROM memory_items WHERE id = ?")
					.get(completed.memoryIds[0]),
			).toEqual({
				source_event_ids_json: '["z-source","a-source"]',
				source_spans_json:
					'[{"eventId":"a-source","startByte":9,"endByte":12},{"eventId":"z-source","startByte":0,"endByte":16}]',
			});
		});

		it("rejects invalid citations and source drift with zero commit", () => {
			const streamId = "claim-bound-invalid-spans";
			capture(store, "utf8-source", { text: "猫" }, { opencodeSessionId: streamId });
			capture(store, "ascii-source", { text: "abc" }, { opencodeSessionId: streamId });
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			const claim = claimFlushJob(store, admission.jobId as number);
			if (!claim) throw new Error("expected invalid-span claim");
			const sessionId = insertTestSession(store.db);
			const before = store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get();
			const invalidCitations = [
				[{ source: 2, start: null, end: null }],
				[
					{ source: 0, start: null, end: null },
					{ source: 0, start: 9, end: 12 },
				],
				[
					{ source: 1, start: null, end: null },
					{ source: 0, start: null, end: null },
				],
				[{ source: 0, start: 9, end: null }],
				[{ source: 0, start: 9, end: 99 }],
				[{ source: 0, start: 10, end: 12 }],
			];
			for (const [index, sourceCitations] of invalidCitations.entries()) {
				expect(() =>
					store.completeRawEventFlushJobMemory(
						{
							claim,
							sourceEventIds: ["utf8-source", "ascii-source"],
							observedOutputCount: 1,
							diagnostic: {},
						},
						(_newMemoryIdFloor, derivation) => [
							derivation.remember({
								sourceCitations,
								sessionId,
								kind: "discovery",
								title: `Invalid citation ${index}`,
								bodyText: "Body",
							}),
						],
					),
				).toThrow(/citation|span|source|order/i);
				expect(store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get()).toEqual(
					before,
				);
			}

			store.db
				.prepare("UPDATE raw_events SET sensitivity = 'private' WHERE event_id = 'utf8-source'")
				.run();
			expect(() =>
				store.completeRawEventFlushJobMemory(
					{
						claim,
						sourceEventIds: ["utf8-source", "ascii-source"],
						observedOutputCount: 1,
						diagnostic: {},
					},
					(_newMemoryIdFloor, derivation) => [
						derivation.remember({
							sourceCitations: [{ source: 0, start: null, end: null }],
							sessionId,
							kind: "discovery",
							title: "Drift",
							bodyText: "Drift",
						}),
					],
				),
			).toThrow(/source set|drift/i);
			expect(store.db.prepare("SELECT COUNT(*) AS count FROM memory_items").get()).toEqual(before);
			expect(
				store.db
					.prepare("SELECT status FROM raw_event_flush_batches WHERE id = ?")
					.get(claim.jobId),
			).toEqual({ status: "processing" });
		});

		it("deduplicates exact retry anchors, rejects active overlap, suppresses tombstones, and permits siblings", () => {
			const createAnchor = (streamId: string, title: string) => {
				const eventId = `${streamId}-event`;
				capture(store, eventId, { text: "abcdefghij" }, { opencodeSessionId: streamId });
				const admission = store.admitRawEventFlushJob({
					source: "codex",
					streamId,
					manifestFingerprint,
					providerFingerprint,
				});
				const claim = claimFlushJob(store, admission.jobId as number);
				if (!claim) throw new Error("expected anchor claim");
				const sessionId = insertTestSession(store.db);
				const complete = (
					activeClaim: typeof claim,
					start: number,
					end: number,
					revisionTitle = title,
				) =>
					store.completeRawEventFlushJobMemory(
						{
							claim: activeClaim,
							sourceEventIds: [eventId],
							observedOutputCount: 1,
							diagnostic: {},
						},
						(_newMemoryIdFloor, derivation) => [
							derivation.remember({
								sourceCitations: [{ source: 0, start, end }],
								sessionId,
								kind: "discovery",
								title: revisionTitle,
								bodyText: "Body",
							}),
						],
					);
				const first = complete(claim, 9, 14);
				return { claim, complete, first };
			};

			const exact = createAnchor("anchor-exact", "Exact anchor");
			const exactRetry = exact.complete(reopenCompletedFlushJob(exact.claim), 9, 14);
			expect(exactRetry.memoryIds).toEqual(exact.first.memoryIds);
			expect(
				store.db
					.prepare("SELECT COUNT(*) AS count FROM memory_items WHERE source_spans_json IS NOT NULL")
					.get(),
			).toEqual({ count: 1 });

			const active = createAnchor("anchor-active-overlap", "Active anchor");
			const activeRetry = reopenCompletedFlushJob(active.claim);
			expect(() => active.complete(activeRetry, 12, 17, "Overlapping anchor")).toThrow(
				/overlap|anchor/i,
			);

			const tombstoned = createAnchor("anchor-tombstone", "Tombstoned anchor");
			store.forget(tombstoned.first.memoryIds[0] as number);
			const suppressed = tombstoned.complete(
				reopenCompletedFlushJob(tombstoned.claim),
				12,
				17,
				"Suppressed anchor",
			);
			expect(suppressed.memoryIds).toEqual([]);

			const sibling = createAnchor("anchor-disjoint", "First sibling");
			const second = sibling.complete(
				reopenCompletedFlushJob(sibling.claim),
				14,
				19,
				"Second sibling",
			);
			expect(second.memoryIds[0]).not.toBe(sibling.first.memoryIds[0]);
		});

		it("suppresses tombstoned re-derivations that cite a different event subset", () => {
			const streamId = "tombstone-subset";
			const eventA = `${streamId}-a`;
			const eventB = `${streamId}-b`;
			capture(store, eventA, { text: "abcdefghij" }, { opencodeSessionId: streamId });
			capture(store, eventB, { text: "klmnopqrst" }, { opencodeSessionId: streamId });
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			const claim = claimFlushJob(store, admission.jobId as number);
			if (!claim) throw new Error("expected subset claim");
			const sessionId = insertTestSession(store.db);
			const complete = (
				activeClaim: typeof claim,
				citations: Array<{ source: number; start: number; end: number }>,
				title: string,
			) =>
				store.completeRawEventFlushJobMemory(
					{
						claim: activeClaim,
						sourceEventIds: [eventA, eventB],
						observedOutputCount: 1,
						diagnostic: {},
					},
					(_newMemoryIdFloor, derivation) => [
						derivation.remember({
							sourceCitations: citations,
							sessionId,
							kind: "discovery",
							title,
							bodyText: "Body",
						}),
					],
				);

			const first = complete(
				claim,
				[
					{ source: 0, start: 9, end: 14 },
					{ source: 1, start: 9, end: 14 },
				],
				"Two-event anchor",
			);
			store.forget(first.memoryIds[0] as number);

			// Gate fires: citing only a SUBSET of the tombstoned events with
			// overlapping bytes must still be suppressed — event-set equality
			// would let this resurrect deleted content.
			const subset = complete(
				reopenCompletedFlushJob(claim),
				[{ source: 0, start: 12, end: 17 }],
				"Subset re-derivation",
			);
			expect(subset.memoryIds).toEqual([]);

			// Gate passes: byte-disjoint content on the same event is a sibling.
			const disjoint = complete(
				reopenCompletedFlushJob(claim),
				[{ source: 0, start: 14, end: 19 }],
				"Disjoint sibling",
			);
			expect(disjoint.memoryIds).toHaveLength(1);
		});

		it("rejects an active-anchor overlap even when the citation adds an unrelated event", () => {
			const streamId = "superset-active";
			const eventA = `${streamId}-a`;
			const eventB = `${streamId}-b`;
			capture(store, eventA, { text: "abcdefghij" }, { opencodeSessionId: streamId });
			capture(store, eventB, { text: "klmnopqrst" }, { opencodeSessionId: streamId });
			const admission = store.admitRawEventFlushJob({
				source: "codex",
				streamId,
				manifestFingerprint,
				providerFingerprint,
			});
			const claim = claimFlushJob(store, admission.jobId as number);
			if (!claim) throw new Error("expected superset claim");
			const sessionId = insertTestSession(store.db);
			const complete = (
				activeClaim: typeof claim,
				citations: Array<{ source: number; start: number; end: number }>,
				title: string,
			) =>
				store.completeRawEventFlushJobMemory(
					{
						claim: activeClaim,
						sourceEventIds: [eventA, eventB],
						observedOutputCount: 1,
						diagnostic: {},
					},
					(_newMemoryIdFloor, derivation) => [
						derivation.remember({
							sourceCitations: citations,
							sessionId,
							kind: "discovery",
							title,
							bodyText: "Body",
						}),
					],
				);

			const first = complete(claim, [{ source: 0, start: 9, end: 14 }], "Single-event anchor");
			expect(first.memoryIds).toHaveLength(1);

			// Padding the citation with an unrelated event must not slip an
			// overlapping anchor past the active-overlap rejection: ambiguous
			// overlap rejects; only disjoint spans may become siblings.
			expect(() =>
				complete(
					reopenCompletedFlushJob(claim),
					[
						{ source: 0, start: 9, end: 12 },
						{ source: 1, start: 9, end: 14 },
					],
					"Superset bypass attempt",
				),
			).toThrow(/overlap|anchor/i);
		});

		it("floors derived sensitivity over the full projected set, not just cited ordinals", () => {
			const runFloorCase = (streamId: string, secondSensitivity: "eligible" | "local_only") => {
				capture(
					store,
					`${streamId}-cited`,
					{ text: "cited text" },
					{ opencodeSessionId: streamId, sensitivity: "eligible" },
				);
				capture(
					store,
					`${streamId}-uncited`,
					{ text: "uncited text" },
					{ opencodeSessionId: streamId, sensitivity: secondSensitivity },
				);
				const admission = store.admitRawEventFlushJob({
					source: "codex",
					streamId,
					manifestFingerprint: localProviderManifest.configurationFingerprint,
					providerFingerprint: localProviderManifest.summaryProvider.providerFingerprint,
				});
				const claim = claimFlushJob(store, admission.jobId as number, localProviderManifest);
				if (!claim) throw new Error("expected floor claim");
				const sessionId = insertTestSession(store.db);
				const completion = store.completeRawEventFlushJobMemory(
					{
						claim,
						sourceEventIds: [`${streamId}-cited`, `${streamId}-uncited`],
						observedOutputCount: 1,
						diagnostic: {},
					},
					(_newMemoryIdFloor, derivation) => [
						derivation.remember({
							// Cites ONLY the eligible ordinal — the restricted source is
							// still in the provider's prompt, so it must still floor.
							sourceCitations: [{ source: 0, start: null, end: null }],
							sessionId,
							kind: "discovery",
							title: `Floor ${streamId}`,
							bodyText: "Body",
						}),
					],
				);
				return store.db
					.prepare("SELECT sensitivity FROM memory_items WHERE id = ?")
					.get(completion.memoryIds[0]);
			};

			// Gate fires: an uncited restricted source floors the derived memory.
			expect(runFloorCase("floor-restricted", "local_only")).toEqual({
				sensitivity: "local_only",
			});
			// Gate passes: an all-eligible projected set stays eligible.
			expect(runFloorCase("floor-eligible", "eligible")).toEqual({ sensitivity: "eligible" });
		});

		it("replays a fanout signal using its stored global producer identity", () => {
			const nextManifest = `sha256:${"d".repeat(64)}`;
			const nextProvider = `sha256:${"e".repeat(64)}`;
			const jobId = insertRetryExhaustedJob("global-producer-replay");
			const fanout = store.importActivationReceipt({
				receiptId: "global-producer-replay-receipt",
				activationSequence: 42,
				manifestFingerprint: nextManifest,
				providerFingerprint: nextProvider,
			});
			expect(fanout).toMatchObject({ disposition: "accepted", fanoutCount: 1 });
			const signal = store.db
				.prepare(
					`SELECT signal_id, producer_receipt_id, sequence, kind,
						target_manifest_fingerprint, target_provider_fingerprint
					 FROM processing_resume_signals WHERE job_id = ?`,
				)
				.get(jobId) as {
				signal_id: string;
				producer_receipt_id: string;
				sequence: number;
				kind: "validated_configuration_activation";
				target_manifest_fingerprint: string;
				target_provider_fingerprint: string;
			};

			expect(
				store.applyResumeSignal({
					signalId: signal.signal_id,
					producerReceiptId: signal.producer_receipt_id,
					targetJobId: jobId,
					sequence: Number(signal.sequence),
					kind: signal.kind,
					targetRole: "summary",
					providerFingerprint: signal.target_provider_fingerprint,
					manifestFingerprint: signal.target_manifest_fingerprint,
				}),
			).toMatchObject({ disposition: "duplicate", grantState: "pending" });
		});

		it("rejects non-monotonic activation sequences before durable fanout", () => {
			const nextManifest = `sha256:${"d".repeat(64)}`;
			const nextProvider = `sha256:${"e".repeat(64)}`;
			const ordinaryStreamId = "activation-sequence-ordinary-queued";
			capture(store, `${ordinaryStreamId}-0`, {}, { opencodeSessionId: ordinaryStreamId });
			const ordinaryJobId = store.admitRawEventFlushJob({
				source: "codex",
				streamId: ordinaryStreamId,
				manifestFingerprint,
				providerFingerprint,
			}).jobId as number;
			expect(
				store.importActivationReceipt({
					receiptId: "activation-sequence-10",
					activationSequence: 10,
					manifestFingerprint: nextManifest,
					providerFingerprint: nextProvider,
				}),
			).toMatchObject({ disposition: "accepted", fanoutCount: 0 });
			expect(
				store.db
					.prepare("SELECT status, resume_grant_state FROM raw_event_flush_batches WHERE id = ?")
					.get(ordinaryJobId),
			).toEqual({ status: "queued", resume_grant_state: "none" });
			const snapshot = () => ({
				receipts: store.db
					.prepare(
						`SELECT receipt_id, producer_sequence, target_job_ids_json
						 FROM processing_resume_producer_receipts
						 ORDER BY producer_sequence, receipt_id`,
					)
					.all(),
				signals: store.db.prepare("SELECT * FROM processing_resume_signals").all(),
			});
			const afterTen = snapshot();
			const futureJobId = insertRetryExhaustedJob("activation-sequence-future-job");
			expect(() =>
				store.applyResumeSignal({
					signalId: "activation-sequence-future-signal",
					producerReceiptId: "activation-sequence-10",
					targetJobId: futureJobId,
					sequence: 10,
					kind: "validated_configuration_activation",
					targetRole: "summary",
					providerFingerprint: nextProvider,
					manifestFingerprint: nextManifest,
				}),
			).toThrow(/not bound to this job/i);

			for (const [receiptId, activationSequence] of [
				["activation-sequence-9", 9],
				["activation-sequence-10-alias", 10],
			] as const) {
				expect(
					store.importActivationReceipt({
						receiptId,
						activationSequence,
						manifestFingerprint: nextManifest,
						providerFingerprint: nextProvider,
					}),
				).toMatchObject({ disposition: "stale", fanoutCount: 0, results: [] });
				expect(snapshot()).toEqual(afterTen);
			}
			expect(
				store.importActivationReceipt({
					receiptId: "activation-sequence-10",
					activationSequence: 10,
					manifestFingerprint: nextManifest,
					providerFingerprint: nextProvider,
				}),
			).toMatchObject({ disposition: "duplicate", fanoutCount: 0 });
			expect(
				store.db
					.prepare("SELECT status, resume_grant_state FROM raw_event_flush_batches WHERE id = ?")
					.get(futureJobId),
			).toEqual({ status: "retry_exhausted", resume_grant_state: "none" });
			expect(
				store.db
					.prepare("SELECT COUNT(*) AS count FROM processing_resume_signals WHERE job_id = ?")
					.get(futureJobId),
			).toEqual({ count: 0 });
			expect(
				store.importActivationReceipt({
					receiptId: "activation-sequence-11",
					activationSequence: 11,
					manifestFingerprint: nextManifest,
					providerFingerprint: nextProvider,
				}),
			).toMatchObject({ disposition: "accepted", fanoutCount: 1 });
			expect(
				store.db
					.prepare("SELECT status, resume_grant_state FROM raw_event_flush_batches WHERE id = ?")
					.get(ordinaryJobId),
			).toEqual({ status: "queued", resume_grant_state: "none" });
		});

		it("does not replay an older pending activation after a newer sequence", () => {
			const jobId = insertRetryExhaustedJob("activation-sequence-superseded");
			const siblingJobId = insertRetryExhaustedJob("activation-sequence-superseded-sibling");
			const newerManifest = testProviderManifest("activation-sequence-newer");
			expect(
				store.applyResumeSignal({
					signalId: "activation-sequence-blocker-signal",
					producerReceiptId: "activation-sequence-blocker-receipt",
					targetJobId: jobId,
					sequence: 1,
					kind: "user_confirmed_doctor_retry",
					targetRole: "summary",
					providerFingerprint,
					manifestFingerprint,
				}),
			).toMatchObject({ disposition: "accepted", grantState: "pending" });
			const older = {
				receiptId: "activation-sequence-older-pending",
				activationSequence: 50,
				manifestFingerprint: `sha256:${"d".repeat(64)}`,
				providerFingerprint: `sha256:${"e".repeat(64)}`,
			};
			const newer = {
				receiptId: "activation-sequence-newer-pending",
				activationSequence: 51,
				manifestFingerprint: newerManifest.configurationFingerprint,
				providerFingerprint: newerManifest.summaryProvider.providerFingerprint,
			};
			expect(store.importActivationReceipt(older)).toMatchObject({
				disposition: "grant_pending",
				fanoutCount: 1,
				results: [{ jobId: siblingJobId, disposition: "accepted" }],
			});
			expect(store.importActivationReceipt(newer)).toMatchObject({
				disposition: "grant_pending",
				fanoutCount: 0,
			});
			const blockerClaim = claimFlushJob(store, jobId);
			if (!blockerClaim) throw new Error("expected blocker claim");
			expect(
				store.failRawEventFlushJob({
					jobId,
					claimGeneration: blockerClaim.claimGeneration,
					attemptFingerprint: blockerClaim.attemptFingerprint,
					safeErrorCode: "provider_unavailable",
				}),
			).toEqual({ status: "retry_exhausted" });
			expect(store.importActivationReceipt(newer)).toMatchObject({
				disposition: "grant_pending",
				fanoutCount: 1,
			});
			const newerClaim = claimFlushJob(store, jobId, newerManifest);
			if (!newerClaim) throw new Error("expected newer activation claim");
			expect(
				store.failRawEventFlushJob({
					jobId,
					claimGeneration: newerClaim.claimGeneration,
					attemptFingerprint: newerClaim.attemptFingerprint,
					safeErrorCode: "provider_unavailable",
				}),
			).toEqual({ status: "retry_exhausted" });

			expect(store.importActivationReceipt(older)).toMatchObject({
				disposition: "stale",
				fanoutCount: 1,
				results: [],
			});
			expect(
				store.db
					.prepare(
						`SELECT COUNT(*) AS count FROM processing_resume_signals
						 WHERE producer_receipt_id = ? AND job_id = ?`,
					)
					.get(older.receiptId, jobId),
			).toEqual({ count: 0 });
		});

		it("keeps a completed older activation duplicate after a newer sequence", () => {
			insertRetryExhaustedJob("activation-sequence-completed-before-newer");
			const older = {
				receiptId: "activation-sequence-completed-older",
				activationSequence: 70,
				manifestFingerprint: `sha256:${"d".repeat(64)}`,
				providerFingerprint: `sha256:${"e".repeat(64)}`,
			};
			expect(store.importActivationReceipt(older)).toMatchObject({
				disposition: "accepted",
				fanoutCount: 1,
			});
			expect(
				store.importActivationReceipt({
					receiptId: "activation-sequence-completed-newer",
					activationSequence: 71,
					manifestFingerprint: `sha256:${"f".repeat(64)}`,
					providerFingerprint: `sha256:${"0".repeat(64)}`,
				}),
			).toMatchObject({ disposition: "grant_pending", fanoutCount: 0 });
			expect(store.importActivationReceipt(older)).toMatchObject({
				disposition: "duplicate",
				fanoutCount: 1,
				results: [],
			});
		});

		it("does not grant any resume producer when the exhausted source range has a gap", () => {
			const streamId = "resume-source-gap";
			const jobId = insertRetryExhaustedJob(streamId);
			store.db.prepare("DELETE FROM raw_events WHERE stream_id = ?").run(streamId);
			const nextManifest = `sha256:${"d".repeat(64)}`;
			const nextProvider = `sha256:${"e".repeat(64)}`;

			expect(
				store.importActivationReceipt({
					receiptId: "source-gap-activation",
					activationSequence: 7,
					manifestFingerprint: nextManifest,
					providerFingerprint: nextProvider,
				}),
			).toMatchObject({ disposition: "accepted", fanoutCount: 0, results: [] });
			expect(
				store.recordProviderHealth({
					manifestFingerprint,
					providerFingerprint,
					health: "provider_unavailable",
				}),
			).toBeNull();
			expect(
				store.recordProviderHealth({
					manifestFingerprint,
					providerFingerprint,
					health: "available",
				}),
			).toMatchObject({ disposition: "accepted", fanoutCount: 0, results: [] });

			let error: unknown;
			try {
				store.confirmDoctorRetry({
					jobId,
					producerReceiptId: "source-gap-doctor",
					expectedRole: "summary",
					expectedProviderFingerprint: providerFingerprint,
					expectedManifestFingerprint: manifestFingerprint,
					expectedAttemptCount: 3,
					expectedClaimGeneration: 0,
					targetProviderFingerprint: providerFingerprint,
					targetManifestFingerprint: manifestFingerprint,
				});
			} catch (caught) {
				error = caught;
			}
			expect(error).toBeInstanceOf(ProcessingResumeError);
			expect(error).toMatchObject({ code: "stale_snapshot", retryable: false });
			expect(
				store.db
					.prepare(
						`SELECT status, resume_grant_state, attempt_count
						 FROM raw_event_flush_batches WHERE id = ?`,
					)
					.get(jobId),
			).toEqual({ status: "retry_exhausted", resume_grant_state: "none", attempt_count: 3 });
		});

		it("rejects an activation receipt reused by a doctor retry without mutating the target job", () => {
			const nextManifest = `sha256:${"d".repeat(64)}`;
			const nextProvider = `sha256:${"e".repeat(64)}`;
			insertRetryExhaustedJob("activation-receipt-source");
			const doctorJobId = insertRetryExhaustedJob(
				"activation-receipt-doctor-target",
				nextManifest,
				nextProvider,
			);
			store.importActivationReceipt({
				receiptId: "activation-receipt-reused-by-doctor",
				activationSequence: 42,
				manifestFingerprint: nextManifest,
				providerFingerprint: nextProvider,
			});
			const snapshot = () => ({
				receipt: store.db
					.prepare(
						`SELECT producer_kind, configuration_fingerprint, provider_fingerprint,
							producer_sequence, fanout_count
						 FROM processing_resume_producer_receipts WHERE receipt_id = ?`,
					)
					.get("activation-receipt-reused-by-doctor"),
				signals: store.db
					.prepare(
						`SELECT signal_id, job_id, sequence, kind, disposition, grant_id
						 FROM processing_resume_signals WHERE producer_receipt_id = ? ORDER BY job_id`,
					)
					.all("activation-receipt-reused-by-doctor"),
				job: store.db
					.prepare(
						`SELECT status, resume_grant_id, resume_grant_state, resume_grant_reason,
							last_resume_signal_id, last_resume_sequence, last_resume_signal_disposition
						 FROM raw_event_flush_batches WHERE id = ?`,
					)
					.get(doctorJobId),
			});
			const before = snapshot();

			let error: unknown;
			try {
				store.confirmDoctorRetry({
					jobId: doctorJobId,
					producerReceiptId: "activation-receipt-reused-by-doctor",
					expectedRole: "summary",
					expectedProviderFingerprint: nextProvider,
					expectedManifestFingerprint: nextManifest,
					expectedAttemptCount: 3,
					expectedClaimGeneration: 0,
					targetProviderFingerprint: nextProvider,
					targetManifestFingerprint: nextManifest,
				});
			} catch (caught) {
				error = caught;
			}
			expect(error).toBeInstanceOf(ProcessingResumeError);
			expect(error).toMatchObject({ code: "invalid_signal", retryable: false });
			expect(snapshot()).toEqual(before);
		});

		it("rejects mismatched fanout reuse of a doctor receipt without mutation", () => {
			const doctorJobId = insertRetryExhaustedJob("doctor-receipt-fanout-source");
			store.confirmDoctorRetry({
				jobId: doctorJobId,
				producerReceiptId: "doctor-receipt-reused-by-activation",
				expectedRole: "summary",
				expectedProviderFingerprint: providerFingerprint,
				expectedManifestFingerprint: manifestFingerprint,
				expectedAttemptCount: 3,
				expectedClaimGeneration: 0,
				targetProviderFingerprint: providerFingerprint,
				targetManifestFingerprint: manifestFingerprint,
			});
			const snapshot = () => ({
				receipts: store.db
					.prepare("SELECT * FROM processing_resume_producer_receipts ORDER BY receipt_id")
					.all(),
				signals: store.db
					.prepare("SELECT * FROM processing_resume_signals ORDER BY signal_id")
					.all(),
				jobs: store.db
					.prepare(
						`SELECT id, status, resume_grant_id, resume_grant_state, resume_grant_reason,
							last_resume_signal_id, last_resume_sequence, last_resume_signal_disposition
						 FROM raw_event_flush_batches ORDER BY id`,
					)
					.all(),
			});
			const before = snapshot();

			let error: unknown;
			try {
				store.importActivationReceipt({
					receiptId: "doctor-receipt-reused-by-activation",
					activationSequence: 9,
					manifestFingerprint: `sha256:${"f".repeat(64)}`,
					providerFingerprint: `sha256:${"0".repeat(64)}`,
				});
			} catch (caught) {
				error = caught;
			}
			expect(error).toBeInstanceOf(ProcessingResumeError);
			expect(error).toMatchObject({ code: "invalid_signal", retryable: false });
			expect(snapshot()).toEqual(before);
		});

		it("rejects reuse of one doctor receipt across jobs with matching producer metadata", () => {
			const firstJobId = insertRetryExhaustedJob("doctor-receipt-first-job");
			const secondJobId = insertRetryExhaustedJob("doctor-receipt-second-job");
			const confirmation = {
				producerReceiptId: "doctor-receipt-cross-job",
				expectedRole: "summary" as const,
				expectedProviderFingerprint: providerFingerprint,
				expectedManifestFingerprint: manifestFingerprint,
				expectedAttemptCount: 3,
				expectedClaimGeneration: 0,
				targetProviderFingerprint: providerFingerprint,
				targetManifestFingerprint: manifestFingerprint,
			};
			store.confirmDoctorRetry({ jobId: firstJobId, ...confirmation });
			const before = store.db
				.prepare(
					`SELECT status, resume_grant_id, resume_grant_state, resume_grant_reason,
						last_resume_signal_id, last_resume_sequence, last_resume_signal_disposition
					 FROM raw_event_flush_batches WHERE id = ?`,
				)
				.get(secondJobId);

			let error: unknown;
			try {
				store.confirmDoctorRetry({ jobId: secondJobId, ...confirmation });
			} catch (caught) {
				error = caught;
			}
			expect(error).toBeInstanceOf(ProcessingResumeError);
			expect(error).toMatchObject({ code: "invalid_signal", retryable: false });
			expect(
				store.db
					.prepare(
						`SELECT status, resume_grant_id, resume_grant_state, resume_grant_reason,
							last_resume_signal_id, last_resume_sequence, last_resume_signal_disposition
						 FROM raw_event_flush_batches WHERE id = ?`,
					)
					.get(secondJobId),
			).toEqual(before);
			expect(
				store.db
					.prepare(
						"SELECT COUNT(*) AS count FROM processing_resume_signals WHERE producer_receipt_id = ?",
					)
					.get("doctor-receipt-cross-job"),
			).toEqual({ count: 1 });
		});

		it("fans one producer receipt out with independent per-job sequences", () => {
			const now = "2026-08-31T00:00:00.000Z";
			capture(store, "fanout-high-0", {}, { opencodeSessionId: "fanout-high" });
			capture(store, "fanout-low-0", {}, { opencodeSessionId: "fanout-low" });
			const insert = store.db.prepare(
				`INSERT INTO raw_event_flush_batches(
					source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, attempt_count, attempt_manifest_fingerprint,
					attempt_provider_fingerprint, last_resume_sequence, created_at, updated_at
				 ) VALUES ('codex', ?, ?, 0, 0, 'raw_events_v1', 'retry_exhausted', 3, ?, ?, ?, ?, ?)`,
			);
			const first = Number(
				insert.run(
					"fanout-high",
					"fanout-high",
					manifestFingerprint,
					providerFingerprint,
					5,
					now,
					now,
				).lastInsertRowid,
			);
			const second = Number(
				insert.run(
					"fanout-low",
					"fanout-low",
					manifestFingerprint,
					providerFingerprint,
					0,
					now,
					now,
				).lastInsertRowid,
			);
			const nextManifest = `sha256:${"e".repeat(64)}`;
			const nextProvider = `sha256:${"f".repeat(64)}`;
			const fanout = store.importActivationReceipt({
				receiptId: "fanout-receipt",
				activationSequence: 2,
				manifestFingerprint: nextManifest,
				providerFingerprint: nextProvider,
			});
			expect(fanout).toMatchObject({ disposition: "accepted", fanoutCount: 2 });
			expect(
				store.db
					.prepare(
						"SELECT job_id, sequence FROM processing_resume_signals WHERE producer_receipt_id = 'fanout-receipt' ORDER BY job_id",
					)
					.all(),
			).toEqual([
				{ job_id: first, sequence: 6 },
				{ job_id: second, sequence: 1 },
			]);
			expect(
				store.db
					.prepare(
						"SELECT producer_sequence FROM processing_resume_producer_receipts WHERE receipt_id = 'fanout-receipt'",
					)
					.get(),
			).toEqual({ producer_sequence: 2 });
		});

		it("fans out past pending jobs and retries only the missing targets", () => {
			const firstJobId = insertRetryExhaustedJob("fanout-pending-first");
			const secondJobId = insertRetryExhaustedJob("fanout-pending-second");
			expect(
				store.applyResumeSignal({
					signalId: "fanout-existing-doctor-signal",
					producerReceiptId: "fanout-existing-doctor-receipt",
					targetJobId: firstJobId,
					sequence: 1,
					kind: "user_confirmed_doctor_retry",
					targetRole: "summary",
					providerFingerprint,
					manifestFingerprint,
				}),
			).toMatchObject({ disposition: "accepted", grantState: "pending" });
			const nextManifest = `sha256:${"e".repeat(64)}`;
			const nextProvider = `sha256:${"f".repeat(64)}`;
			const activation = {
				receiptId: "fanout-partial-activation",
				activationSequence: 50,
				manifestFingerprint: nextManifest,
				providerFingerprint: nextProvider,
			};

			expect(store.importActivationReceipt(activation)).toMatchObject({
				disposition: "grant_pending",
				fanoutCount: 1,
				results: [{ jobId: secondJobId, disposition: "accepted" }],
			});
			const firstClaim = claimFlushJob(store, firstJobId);
			if (!firstClaim) throw new Error("expected pending doctor claim");
			expect(store.importActivationReceipt(activation)).toMatchObject({
				disposition: "grant_pending",
				fanoutCount: 1,
				results: [],
			});
			expect(
				store.failRawEventFlushJob({
					jobId: firstJobId,
					claimGeneration: firstClaim.claimGeneration,
					attemptFingerprint: firstClaim.attemptFingerprint,
					safeErrorCode: "provider_unavailable",
				}),
			).toEqual({ status: "retry_exhausted" });

			expect(store.importActivationReceipt(activation)).toMatchObject({
				disposition: "accepted",
				fanoutCount: 2,
				results: [{ jobId: firstJobId, disposition: "accepted" }],
			});
			expect(
				store.db
					.prepare(
						`SELECT job_id FROM processing_resume_signals
						 WHERE producer_receipt_id = ? ORDER BY job_id`,
					)
					.all(activation.receiptId),
			).toEqual([{ job_id: firstJobId }, { job_id: secondJobId }]);
			expect(store.importActivationReceipt(activation)).toMatchObject({
				disposition: "duplicate",
				fanoutCount: 2,
				results: [],
			});
		});

		it("records a durable no-op when a frozen target changes before replay", () => {
			const jobId = insertRetryExhaustedJob("fanout-frozen-attempt-change");
			const nextProviderManifest = testProviderManifest("fanout-frozen-next");
			const nextManifest = nextProviderManifest.configurationFingerprint;
			const nextProvider = nextProviderManifest.summaryProvider.providerFingerprint;
			expect(
				store.importActivationReceipt({
					receiptId: "fanout-frozen-first-activation",
					activationSequence: 59,
					providerFingerprint: nextProvider,
					manifestFingerprint: nextManifest,
				}),
			).toMatchObject({ disposition: "accepted", fanoutCount: 1 });
			const activation = {
				receiptId: "fanout-frozen-activation",
				activationSequence: 60,
				manifestFingerprint: nextManifest,
				providerFingerprint: nextProvider,
			};
			expect(store.importActivationReceipt(activation)).toMatchObject({
				disposition: "grant_pending",
				fanoutCount: 0,
			});
			const claim = claimFlushJob(store, jobId, nextProviderManifest);
			if (!claim) throw new Error("expected frozen target claim");
			expect(
				store.failRawEventFlushJob({
					jobId,
					claimGeneration: claim.claimGeneration,
					attemptFingerprint: claim.attemptFingerprint,
					safeErrorCode: "provider_unavailable",
				}),
			).toEqual({ status: "retry_exhausted" });

			expect(store.importActivationReceipt(activation)).toMatchObject({
				disposition: "accepted",
				fanoutCount: 0,
				results: [{ jobId, disposition: "unchanged_configuration" }],
			});
			expect(
				store.db
					.prepare(
						`SELECT disposition FROM processing_resume_signals
						 WHERE job_id = ? AND producer_receipt_id = ?`,
					)
					.get(jobId, activation.receiptId),
			).toEqual({ disposition: "unchanged_configuration" });
			expect(store.importActivationReceipt(activation)).toMatchObject({
				disposition: "duplicate",
				fanoutCount: 0,
				results: [],
			});
		});

		it("purges only completed source below the frontier and exempts every uncompleted range", () => {
			const durable = store;
			const statuses = ["queued", "processing", "failed", "retry_exhausted"] as const;
			for (const streamId of ["completed", "backlog", ...statuses]) {
				capture(
					durable,
					`${streamId}-event`,
					{ streamId },
					{
						opencodeSessionId: streamId,
						tsWallMs: Date.now() - 60_000,
					},
				);
			}
			store.db
				.prepare(
					"UPDATE raw_event_sessions SET last_flushed_event_seq = 0 WHERE stream_id != 'backlog'",
				)
				.run();
			for (const status of statuses) {
				store.db
					.prepare(
						`INSERT INTO raw_event_flush_batches(
							source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
							extractor_version, status, created_at, updated_at
						 ) VALUES ('codex', ?, ?, 0, 0, 'raw_events_v1', ?, ?, ?)`,
					)
					.run(status, status, status, new Date().toISOString(), new Date().toISOString());
			}

			expect(store.purgeRawEvents(1)).toBe(1);
			expect(store.db.prepare("SELECT stream_id FROM raw_events ORDER BY stream_id").all()).toEqual(
				["backlog", "failed", "processing", "queued", "retry_exhausted"].map((stream_id) => ({
					stream_id,
				})),
			);
		});
	});
});

// ---------------------------------------------------------------------------
// buildFilterClauses (unit tests)
// ---------------------------------------------------------------------------

describe("buildFilterClauses", () => {
	it("returns empty for null/undefined filters", () => {
		const result = buildFilterClauses(null);
		expect(result.clauses).toEqual([]);
		expect(result.params).toEqual([]);
		expect(result.joinSessions).toBe(false);
	});

	it("builds kind filter", () => {
		const result = buildFilterClauses({ kind: "discovery" });
		expect(result.clauses).toEqual(["memory_items.kind = ?"]);
		expect(result.params).toEqual(["discovery"]);
	});

	it("builds include_visibility filter", () => {
		const result = buildFilterClauses({ include_visibility: ["private", "shared"] });
		expect(result.clauses).toHaveLength(1);
		expect(result.clauses[0]).toContain("IN");
		expect(result.params).toEqual(["private", "shared"]);
	});

	it("normalizes visibility/workspace/trust values like Python", () => {
		const result = buildFilterClauses({
			visibility: [" Shared ", "INVALID", "private"],
			include_workspace_kinds: ["PERSONAL", "nope", "shared"],
			include_trust_states: ["Unreviewed", "bogus", "trusted"],
		});
		expect(result.params).toEqual([
			"shared",
			"private",
			"personal",
			"shared",
			"unreviewed",
			"trusted",
		]);
	});

	it("builds exclude_actor_ids filter", () => {
		const result = buildFilterClauses({ exclude_actor_ids: ["actor:123"] });
		expect(result.clauses).toHaveLength(1);
		expect(result.clauses[0]).toContain("NOT IN");
		expect(result.params).toEqual(["actor:123"]);
	});

	it("combines multiple filters", () => {
		const result = buildFilterClauses({
			kind: "feature",
			include_visibility: ["shared"],
			exclude_workspace_kinds: ["personal"],
		});
		expect(result.clauses).toHaveLength(3);
		expect(result.params).toEqual(["feature", "shared", "personal"]);
	});

	it("builds ownership_scope mine clause with actor/device context", () => {
		const result = buildFilterClausesWithContext(
			{ ownership_scope: "mine" },
			{ actorId: "local:device-1", deviceId: "device-1" },
		);
		expect(result.clauses).toHaveLength(1);
		expect(result.clauses[0]).toContain("memory_items.actor_id");
		expect(result.clauses[0]).toContain("$.actor_id");
		expect(result.clauses[0]).toContain("memory_items.origin_device_id");
		expect(result.clauses[0]).toContain("$.origin_device_id");
		expect(result.params).toEqual(["local:device-1", "device-1"]);
	});

	it("builds ownership_scope mine clause with claimed same-actor peers", () => {
		const result = buildFilterClausesWithContext(
			{ ownership_scope: "mine" },
			{
				actorId: "local:device-1",
				deviceId: "device-1",
				claimedDeviceIds: ["peer-a", " peer-a ", "peer-b"],
				legacyActorIds: ["legacy-sync:peer-a"],
			},
		);
		expect(result.clauses).toHaveLength(1);
		expect(result.clauses[0]).toContain("IN (?, ?)");
		expect(result.clauses[0]).toContain("IN (?)");
		expect(result.clauses[0]).toContain("$.actor_id");
		expect(result.clauses[0]).toContain("$.origin_device_id");
		expect(result.params).toEqual([
			"local:device-1",
			"device-1",
			"peer-a",
			"peer-b",
			"legacy-sync:peer-a",
		]);
	});

	it("builds ownership_scope theirs clause with null-safe comparisons", () => {
		const result = buildFilterClausesWithContext(
			{ ownership_scope: "theirs" },
			{ actorId: "local:device-1", deviceId: "device-1" },
		);
		expect(result.clauses).toHaveLength(1);
		expect(result.clauses[0]).toMatch(/^NOT \(/);
		expect(result.clauses[0]).toContain("$.actor_id");
		expect(result.clauses[0]).toContain("$.origin_device_id");
		expect(result.params).toEqual(["local:device-1", "device-1"]);
	});

	it("builds ownership_scope theirs as inverse of claimed same-actor ownership", () => {
		const result = buildFilterClausesWithContext(
			{ ownership_scope: "theirs" },
			{
				actorId: "local:device-1",
				deviceId: "device-1",
				claimedDeviceIds: ["peer-a"],
				legacyActorIds: ["legacy-sync:peer-a"],
			},
		);
		expect(result.clauses).toHaveLength(1);
		expect(result.clauses[0]).toMatch(/^NOT \(/);
		expect(result.clauses[0]).toContain("IN (?)");
		expect(result.clauses[0]).toContain("$.actor_id");
		expect(result.clauses[0]).toContain("$.origin_device_id");
		expect(result.params).toEqual(["local:device-1", "device-1", "peer-a", "legacy-sync:peer-a"]);
	});

	it("does not treat ownerless rows as mine when identity context is blank", () => {
		const result = buildFilterClausesWithContext(
			{ ownership_scope: "mine" },
			{ actorId: "", deviceId: "  " },
		);
		// Blank actor/device must match nothing rather than emitting `<expr> = ''`,
		// which would otherwise own every anonymous row and diverge from
		// MemoryStore.buildOwnershipPredicate().
		expect(result.clauses).toEqual(["(0 = 1)"]);
		expect(result.params).toEqual([]);
	});
});
