import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import Database from "better-sqlite3";
import { describe, expect, it } from "vitest";
import {
	compileRunnerLocalDestinationBoundary,
	compileUntrustedDestinationBoundary,
	type DestinationBoundaryV1,
} from "./destination-boundary.js";
import type { ExportOptions, ExportPayload, ImportOptions } from "./export-import.js";
import { exportMemoriesWithDb, importMemoriesWithDb, readImportPayload } from "./export-import.js";
import { initTestSchema } from "./test-utils.js";

const CONFIGURATION_FINGERPRINT = `sha256:${"a".repeat(64)}`;
const REPOSITORY_A = `repo-v1:sha256:${"1".repeat(64)}`;
const REPOSITORY_B = `repo-v1:sha256:${"2".repeat(64)}`;

function exportMemories(
	opts: ExportOptions,
	destinationBoundary?: DestinationBoundaryV1,
): ExportPayload {
	const db = new Database(opts.dbPath as string);
	try {
		return exportMemoriesWithDb(db, opts, destinationBoundary);
	} finally {
		db.close();
	}
}

function importMemories(
	payload: ExportPayload,
	opts: ImportOptions,
): ReturnType<typeof importMemoriesWithDb> {
	const db = new Database(opts.dbPath as string);
	try {
		return importMemoriesWithDb(db, payload, opts);
	} finally {
		db.close();
	}
}

function createDbPath(name: string): string {
	const dir = mkdtempSync(join(tmpdir(), "codemem-export-import-"));
	return join(dir, `${name}.sqlite`);
}

function seedSourceDb(dbPath: string): void {
	const db = new Database(dbPath);
	try {
		initTestSchema(db);
		db.prepare(
			`INSERT INTO sessions(id, started_at, cwd, project, user, tool_version, metadata_json, import_key, repository_identity)
			 VALUES (1, '2026-03-01T10:00:00Z', '/tmp/repo', 'codemem', 'adam', 'test', '{"k":1}', 'sess-1', ?)`,
		).run(REPOSITORY_A);
		db.prepare(
			`INSERT INTO user_prompts(id, session_id, project, prompt_text, prompt_number, created_at, created_at_epoch, metadata_json, import_key, sensitivity, repository_identity)
			 VALUES (10, 1, 'codemem', 'Run tests', 1, '2026-03-01T10:01:00Z', 1, '{"p":1}', 'prompt-1', 'eligible', ?)`,
		).run(REPOSITORY_A);
		db.prepare(
			`INSERT INTO memory_items(
				id, session_id, kind, title, body_text, confidence, tags_text, active,
				created_at, updated_at, metadata_json, facts, concepts, files_read, files_modified,
				user_prompt_id, prompt_number, import_key, sensitivity, repository_identity
			) VALUES (
				100, 1, 'feature', 'Added export', 'implemented export', 0.9, 'ts export', 1,
				'2026-03-01T10:02:00Z', '2026-03-01T10:02:00Z', '{"m":1}', '["fact"]', '["concept"]', '["a.ts"]', '["b.ts"]',
				10, 1, 'memory-1', 'eligible', ?
			)`,
		).run(REPOSITORY_A);
		db.prepare(
			`INSERT INTO memory_items(
				id, session_id, kind, title, body_text, confidence, tags_text, active,
				created_at, updated_at, metadata_json, deleted_at, import_key, sensitivity, repository_identity
			) VALUES (
				101, 1, 'exploration', 'Inactive', 'skipped by default', 0.5, '', 0,
				'2026-03-01T10:03:00Z', '2026-03-01T10:03:00Z', '{}', '2026-03-01T10:03:30Z', 'memory-2', 'eligible', ?
			)`,
		).run(REPOSITORY_A);
		db.prepare(
			`INSERT INTO session_summaries(
				id, session_id, project, request, investigated, learned, completed, next_steps,
				notes, files_read, files_edited, prompt_number, created_at, created_at_epoch, metadata_json, import_key,
				sensitivity, repository_identity
			) VALUES (
				200, 1, 'codemem', 'ship export', 'cli parity', 'ts store is thinner', 'ported base', 'port config next',
				'', '["a.ts"]', '["b.ts"]', 1, '2026-03-01T10:04:00Z', 1, '{"s":1}', 'summary-1',
				'eligible', ?
			)`,
		).run(REPOSITORY_A);
	} finally {
		db.close();
	}
}

function grantScope(db: Database.Database, scopeId: string, deviceId = "local"): void {
	const now = "2026-01-01T00:00:00Z";
	db.prepare(
		`INSERT INTO replication_scopes(
			scope_id, label, kind, authority_type, membership_epoch, status, created_at, updated_at
		 ) VALUES (?, ?, 'team', 'coordinator', 1, 'active', ?, ?)`,
	).run(scopeId, scopeId, now, now);
	db.prepare(
		`INSERT INTO scope_memberships(scope_id, device_id, role, status, membership_epoch, updated_at)
		 VALUES (?, ?, 'member', 'active', 1, ?)`,
	).run(scopeId, deviceId, now);
}

function minimalPayload(scopeId: string): ReturnType<typeof exportMemories> {
	return {
		version: "1.0",
		exported_at: "2026-03-01T00:00:00Z",
		export_metadata: {
			tool_version: "codemem",
			projects: ["codemem"],
			total_memories: 1,
			total_sessions: 1,
			include_inactive: false,
			filters: {},
		},
		sessions: [
			{
				id: 1,
				started_at: "2026-03-01T00:00:00Z",
				cwd: "/tmp/codemem",
				project: "codemem",
				user: "test",
				tool_version: "test",
				metadata_json: {},
				import_key: "session-1",
			},
		],
		memory_items: [
			{
				id: 100,
				session_id: 1,
				kind: "discovery",
				title: "Scoped import",
				body_text: "Scoped body",
				created_at: "2026-03-01T00:00:01Z",
				updated_at: "2026-03-01T00:00:01Z",
				metadata_json: {},
				import_key: "memory-100",
				scope_id: scopeId,
			},
		],
		session_summaries: [],
		user_prompts: [],
	};
}

describe("export/import", () => {
	it("exports parsed JSON fields and prompt import key links", () => {
		const dbPath = createDbPath("source");
		seedSourceDb(dbPath);

		const payload = exportMemories({ dbPath, project: "codemem" });

		expect(payload.version).toBe("2.0");
		expect(payload.sessions).toHaveLength(1);
		expect(payload.memory_items).toHaveLength(1);
		expect(payload.session_summaries).toHaveLength(1);
		expect(payload.user_prompts).toHaveLength(1);
		expect(payload.sessions[0]).not.toHaveProperty("metadata_json");
		expect(payload.memory_items[0]?.facts).toEqual(["fact"]);
		expect(payload.memory_items[0]?.scope_id).toBe("local-default");
		expect(payload.memory_items[0]?.user_prompt_import_key).toBe("prompt-1");
	});

	it("exports v2 through one destination boundary and emits only safe session shells", () => {
		const dbPath = createDbPath("destination-boundary");
		const db = new Database(dbPath);
		try {
			initTestSchema(db);
			db.prepare(
				`INSERT INTO sessions(
					id, started_at, ended_at, cwd, project, git_remote, git_branch, user,
					tool_version, metadata_json, import_key, repository_identity
				 ) VALUES
					(1, '2026-03-01T10:00:00Z', NULL, '/restricted/cwd-a', 'display-a',
					 'git@restricted/a.git', 'private-branch-a', 'private-user-a', 'test',
					 '{"PRIVATE_SESSION_METADATA_A":true}', 'session-a', ?),
					(2, '2026-03-01T10:00:00Z', NULL, '/restricted/cwd-b', 'display-b',
					 'git@restricted/b.git', 'private-branch-b', 'private-user-b', 'test',
					 '{"PRIVATE_SESSION_METADATA_B":true}', 'session-b', ?)`,
			).run(REPOSITORY_A, REPOSITORY_B);
			db.prepare(
				`INSERT INTO memory_items(
					id, session_id, kind, title, body_text, active, created_at, updated_at,
					metadata_json, import_key, sensitivity, repository_identity
				 ) VALUES
					(101, 1, 'discovery', 'ELIGIBLE_MEMORY', 'eligible body', 1, ?, ?, '{}', 'memory-eligible', 'eligible', ?),
					(102, 1, 'discovery', 'PRIVATE_MEMORY_A', 'private body a', 1, ?, ?, '{}', 'memory-private-a', 'private', ?),
					(103, 2, 'discovery', 'PRIVATE_MEMORY_B', 'private body b', 1, ?, ?, '{}', 'memory-private-b', 'private', ?),
					(104, 1, 'discovery', 'SECRET_MEMORY', 'secret body', 1, ?, ?, '{}', 'memory-secret', 'secret', ?)`,
			).run(
				"2026-03-01T10:01:00Z",
				"2026-03-01T10:01:00Z",
				REPOSITORY_A,
				"2026-03-01T10:02:00Z",
				"2026-03-01T10:02:00Z",
				REPOSITORY_A,
				"2026-03-01T10:03:00Z",
				"2026-03-01T10:03:00Z",
				REPOSITORY_B,
				"2026-03-01T10:04:00Z",
				"2026-03-01T10:04:00Z",
				REPOSITORY_A,
			);
			db.prepare(
				`INSERT INTO user_prompts(
					id, session_id, project, prompt_text, created_at, created_at_epoch,
					metadata_json, import_key, sensitivity, repository_identity
				 ) VALUES
					(201, 1, 'display-a', 'ELIGIBLE_PROMPT', ?, 1, '{}', 'prompt-eligible', 'eligible', ?),
					(202, 1, 'display-a', 'PRIVATE_PROMPT_A', ?, 2, '{}', 'prompt-private-a', 'private', ?)`,
			).run("2026-03-01T10:05:00Z", REPOSITORY_A, "2026-03-01T10:06:00Z", REPOSITORY_A);
			db.prepare(
				`INSERT INTO session_summaries(
					id, session_id, project, request, created_at, created_at_epoch,
					metadata_json, import_key, sensitivity, repository_identity
				 ) VALUES
					(301, 1, 'display-a', 'ELIGIBLE_SUMMARY', ?, 1, '{}', 'summary-eligible', 'eligible', ?),
					(302, 1, 'display-a', 'LOCAL_ONLY_SUMMARY_A', ?, 2, '{}', 'summary-local-a', 'local_only', ?)`,
			).run("2026-03-01T10:07:00Z", REPOSITORY_A, "2026-03-01T10:08:00Z", REPOSITORY_A);
		} finally {
			db.close();
		}

		const remote = compileUntrustedDestinationBoundary({
			consumer: "export",
			configurationFingerprint: CONFIGURATION_FINGERPRINT,
		});
		const remotePayload = exportMemories({ dbPath, allProjects: true }, remote);
		const defaultPayload = exportMemories({ dbPath, allProjects: true });
		const localPayload = exportMemories(
			{ dbPath, allProjects: true },
			compileRunnerLocalDestinationBoundary({
				consumer: "export",
				configurationFingerprint: CONFIGURATION_FINGERPRINT,
				repositoryIdentity: REPOSITORY_A,
			}),
		);

		expect(remotePayload.version).toBe("2.0");
		expect(remotePayload.memory_items.map((row) => row.title)).toEqual(["ELIGIBLE_MEMORY"]);
		expect(remotePayload.user_prompts.map((row) => row.prompt_text)).toEqual(["ELIGIBLE_PROMPT"]);
		expect(remotePayload.session_summaries.map((row) => row.request)).toEqual(["ELIGIBLE_SUMMARY"]);
		expect(defaultPayload).toMatchObject({
			version: remotePayload.version,
			sessions: remotePayload.sessions,
			memory_items: remotePayload.memory_items,
			session_summaries: remotePayload.session_summaries,
			user_prompts: remotePayload.user_prompts,
		});
		expect(remotePayload.sessions).toHaveLength(1);
		expect(Object.keys(remotePayload.sessions[0] ?? {}).sort()).toEqual(
			[
				"ended_at",
				"id",
				"import_key",
				"project",
				"repository_identity",
				"started_at",
				"tool_version",
			].sort(),
		);
		for (const forbidden of [
			"PRIVATE_MEMORY",
			"SECRET_MEMORY",
			"PRIVATE_PROMPT",
			"LOCAL_ONLY_SUMMARY",
			"/restricted/",
			"git@restricted",
			"private-branch",
			"private-user",
			"PRIVATE_SESSION_METADATA",
		]) {
			expect(JSON.stringify(remotePayload)).not.toContain(forbidden);
		}
		expect(localPayload.memory_items.map((row) => row.title)).toEqual([
			"ELIGIBLE_MEMORY",
			"PRIVATE_MEMORY_A",
		]);
		expect(localPayload.user_prompts.map((row) => row.prompt_text)).toEqual([
			"ELIGIBLE_PROMPT",
			"PRIVATE_PROMPT_A",
		]);
		expect(localPayload.session_summaries.map((row) => row.request)).toEqual([
			"ELIGIBLE_SUMMARY",
			"LOCAL_ONLY_SUMMARY_A",
		]);
		expect(JSON.stringify(localPayload)).not.toContain("PRIVATE_MEMORY_B");
		expect(JSON.stringify(localPayload)).not.toContain("SECRET_MEMORY");
	});

	it("falls back to the local device id when CODEMEM_DEVICE_ID is whitespace", () => {
		const previousDeviceId = process.env.CODEMEM_DEVICE_ID;
		const dbPath = createDbPath("whitespace-device-id");
		const db = new Database(dbPath);
		try {
			initTestSchema(db);
			grantScope(db, "authorized-team");
			db.prepare(
				`INSERT INTO sessions(id, started_at, cwd, project, user, tool_version, metadata_json, import_key)
				 VALUES (1, '2026-03-01T00:00:00Z', '/tmp/visible', 'visible', 'test', 'test', '{}', 'session-visible')`,
			).run();
			db.prepare(
				`INSERT INTO memory_items(
					id, session_id, kind, title, body_text, active, created_at, updated_at, metadata_json, import_key, scope_id
				 ) VALUES (100, 1, 'discovery', 'Visible through local fallback', 'visible', 1, '2026-03-01T00:00:01Z', '2026-03-01T00:00:01Z', '{}', 'memory-visible', 'authorized-team')`,
			).run();
			db.prepare("UPDATE memory_items SET sensitivity = 'eligible' WHERE id = 100").run();
		} finally {
			db.close();
		}
		process.env.CODEMEM_DEVICE_ID = "   ";
		try {
			const payload = exportMemories({ dbPath, allProjects: true });
			// Whitespace must resolve to device "local"; otherwise its team membership
			// cannot authorize this scoped memory and the export becomes empty.
			expect(payload.memory_items.map((memory) => memory.title)).toEqual([
				"Visible through local fallback",
			]);
		} finally {
			if (previousDeviceId === undefined) delete process.env.CODEMEM_DEVICE_ID;
			else process.env.CODEMEM_DEVICE_ID = previousDeviceId;
		}
	});

	it("exports only locally authorized scopes and tags source scope ids", () => {
		const dbPath = createDbPath("scoped-export");
		const db = new Database(dbPath);
		try {
			initTestSchema(db);
			grantScope(db, "authorized-team");
			db.prepare(
				`INSERT INTO replication_scopes(
					scope_id, label, kind, authority_type, membership_epoch, status, created_at, updated_at
				 ) VALUES ('unauthorized-team', 'unauthorized-team', 'team', 'coordinator', 1, 'active', ?, ?)`,
			).run("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z");
			db.prepare(
				`INSERT INTO sessions(id, started_at, cwd, project, user, tool_version, metadata_json, import_key)
				 VALUES (1, '2026-03-01T00:00:00Z', '/tmp/visible', 'visible', 'test', 'test', '{}', 'session-visible'),
						(2, '2026-03-01T00:00:00Z', '/tmp/hidden', 'hidden', 'test', 'test', '{}', 'session-hidden')`,
			).run();
			db.prepare(
				`INSERT INTO memory_items(
					id, session_id, kind, title, body_text, active, created_at, updated_at, metadata_json, import_key, scope_id
				 ) VALUES
					(100, 1, 'discovery', 'Visible scoped export', 'visible', 1, '2026-03-01T00:00:01Z', '2026-03-01T00:00:01Z', '{}', 'memory-visible', 'authorized-team'),
					(101, 2, 'discovery', 'Hidden scoped export', 'hidden', 1, '2026-03-01T00:00:02Z', '2026-03-01T00:00:02Z', '{}', 'memory-hidden', 'unauthorized-team')`,
			).run();
			db.prepare("UPDATE memory_items SET sensitivity = 'eligible' WHERE id = 100").run();
		} finally {
			db.close();
		}

		const payload = exportMemories({ dbPath, allProjects: true });

		expect(payload.sessions.map((session) => session.import_key)).toEqual(["session-visible"]);
		expect(payload.memory_items.map((memory) => memory.title)).toEqual(["Visible scoped export"]);
		expect(payload.memory_items[0]?.scope_id).toBe("authorized-team");
	});

	it("exports null-scope legacy rows as local-default even when project mappings exist", () => {
		const dbPath = createDbPath("mapped-null-scope-export");
		const db = new Database(dbPath);
		try {
			initTestSchema(db);
			grantScope(db, "authorized-team");
			db.prepare(
				`INSERT INTO project_scope_mappings(
					workspace_identity, project_pattern, scope_id, priority, source, created_at, updated_at
				 ) VALUES ('/tmp/mapped', '/tmp/mapped', 'authorized-team', 10, 'user', ?, ?)`,
			).run("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z");
			db.prepare(
				`INSERT INTO sessions(id, started_at, cwd, project, user, tool_version, metadata_json, import_key)
				 VALUES (1, '2026-03-01T00:00:00Z', '/tmp/mapped', 'mapped', 'test', 'test', '{}', 'session-mapped')`,
			).run();
			db.prepare(
				`INSERT INTO memory_items(
					id, session_id, kind, title, body_text, active, created_at, updated_at, metadata_json, import_key, scope_id
				 ) VALUES (100, 1, 'discovery', 'Legacy null scope', 'legacy', 1, '2026-03-01T00:00:01Z', '2026-03-01T00:00:01Z', '{}', 'memory-legacy', NULL)`,
			).run();
			db.prepare("UPDATE memory_items SET sensitivity = 'eligible' WHERE id = 100").run();
		} finally {
			db.close();
		}

		const payload = exportMemories({ dbPath, allProjects: true });

		expect(payload.memory_items).toHaveLength(1);
		expect(payload.memory_items[0]?.scope_id).toBe("local-default");
	});

	it("includes inactive memories when requested", () => {
		const dbPath = createDbPath("inactive");
		seedSourceDb(dbPath);

		const payload = exportMemories({ dbPath, project: "codemem", includeInactive: true });

		expect(payload.memory_items).toHaveLength(2);
	});

	it("imports idempotently and supports dry run", () => {
		const sourcePath = createDbPath("source-import");
		seedSourceDb(sourcePath);
		const payload = exportMemories({
			dbPath: sourcePath,
			project: "codemem",
			includeInactive: true,
		});

		const destPath = createDbPath("dest-import");
		const destDb = new Database(destPath);
		initTestSchema(destDb);
		destDb.close();

		const dryRun = importMemories(payload, { dbPath: destPath, dryRun: true });
		expect(dryRun.dryRun).toBe(true);
		expect(dryRun.sessions).toBe(1);

		const first = importMemories(payload, { dbPath: destPath, remapProject: "/tmp/remapped" });
		expect(first.sessions).toBe(1);
		expect(first.user_prompts).toBe(1);
		expect(first.memory_items).toBe(2);
		expect(first.session_summaries).toBe(1);

		const second = importMemories(payload, { dbPath: destPath, remapProject: "/tmp/remapped" });
		expect(second.sessions).toBe(0);
		expect(second.user_prompts).toBe(0);
		expect(second.memory_items).toBe(0);
		expect(second.session_summaries).toBe(0);

		const checkDb = new Database(destPath, { readonly: true });
		try {
			const promptEpoch = (
				checkDb.prepare("SELECT created_at_epoch FROM user_prompts LIMIT 1").get() as {
					created_at_epoch: number;
				}
			).created_at_epoch;
			const summaryEpoch = (
				checkDb.prepare("SELECT created_at_epoch FROM session_summaries LIMIT 1").get() as {
					created_at_epoch: number;
				}
			).created_at_epoch;
			const counts = {
				sessions: (checkDb.prepare("SELECT COUNT(*) AS n FROM sessions").get() as { n: number }).n,
				prompts: (checkDb.prepare("SELECT COUNT(*) AS n FROM user_prompts").get() as { n: number })
					.n,
				memories: (checkDb.prepare("SELECT COUNT(*) AS n FROM memory_items").get() as { n: number })
					.n,
				summaries: (
					checkDb.prepare("SELECT COUNT(*) AS n FROM session_summaries").get() as { n: number }
				).n,
				project: (
					checkDb.prepare("SELECT project FROM sessions LIMIT 1").get() as { project: string }
				).project,
				inactiveMemory: checkDb
					.prepare("SELECT active, deleted_at FROM memory_items WHERE import_key = ?")
					.get("memory-2") as { active: number; deleted_at: string | null },
				memoryScopes: checkDb
					.prepare("SELECT DISTINCT scope_id FROM memory_items ORDER BY scope_id")
					.all() as Array<{ scope_id: string | null }>,
			};
			expect(counts).toEqual({
				sessions: 1,
				prompts: 1,
				memories: 2,
				summaries: 1,
				project: "/tmp/remapped",
				inactiveMemory: { active: 0, deleted_at: "2026-03-01T10:03:30Z" },
				memoryScopes: [{ scope_id: "local-default" }],
			});
			// Original created_at_epoch values (1) from the source DB are preserved
			expect(promptEpoch).toBe(1);
			expect(summaryEpoch).toBe(1);
		} finally {
			checkDb.close();
		}
	});

	it("preserves valid v2 provenance and fails closed for invalid or legacy content", () => {
		const v2Payload = {
			version: "2.0",
			exported_at: "2026-09-01T00:00:00.000Z",
			export_metadata: {
				tool_version: "codemem",
				projects: ["display-a"],
				total_memories: 2,
				total_sessions: 1,
				include_inactive: false,
				filters: {},
			},
			sessions: [
				{
					id: 1,
					started_at: "2026-09-01T00:00:00.000Z",
					ended_at: null,
					project: "display-a",
					tool_version: "test",
					import_key: "safe-session-a",
					repository_identity: REPOSITORY_A,
				},
			],
			memory_items: [
				{
					id: 100,
					session_id: 1,
					kind: "discovery",
					title: "VALID_PRIVATE_V2",
					body_text: "valid private body",
					created_at: "2026-09-01T00:00:01.000Z",
					updated_at: "2026-09-01T00:00:01.000Z",
					import_key: "v2-private",
					scope_id: "local-default",
					sensitivity: "private",
					repository_identity: REPOSITORY_A,
					lineage_id: "d".repeat(64),
					revision_id: `revision:${"e".repeat(64)}`,
					revision_ordinal: 1,
					derivation_key: "f".repeat(64),
					source_event_ids_json: ["event-v2-private"],
					source_spans_json: [{ eventId: "event-v2-private", startByte: 0, endByte: 18 }],
					manifest_fingerprint: CONFIGURATION_FINGERPRINT,
					provider_fingerprint: `sha256:${"b".repeat(64)}`,
					attempt_fingerprint: `sha256:${"c".repeat(64)}`,
				},
				{
					id: 101,
					session_id: 1,
					kind: "discovery",
					title: "INVALID_V2_PROVENANCE",
					body_text: "invalid provenance body",
					created_at: "2026-09-01T00:00:02.000Z",
					updated_at: "2026-09-01T00:00:02.000Z",
					import_key: "v2-invalid",
					scope_id: "local-default",
					sensitivity: "private",
					repository_identity: "repo-v1:forged",
				},
			],
			user_prompts: [
				{
					id: 200,
					session_id: 1,
					project: "display-a",
					prompt_text: "VALID_ELIGIBLE_PROMPT_V2",
					created_at: "2026-09-01T00:00:03.000Z",
					created_at_epoch: 3,
					import_key: "v2-prompt",
					sensitivity: "eligible",
					repository_identity: REPOSITORY_A,
				},
			],
			session_summaries: [
				{
					id: 300,
					session_id: 1,
					project: "display-a",
					request: "VALID_LOCAL_SUMMARY_V2",
					created_at: "2026-09-01T00:00:04.000Z",
					created_at_epoch: 4,
					import_key: "v2-summary",
					sensitivity: "local_only",
					repository_identity: REPOSITORY_A,
				},
			],
		} as unknown as ExportPayload;
		const v2DbPath = createDbPath("v2-provenance");
		const v2Db = new Database(v2DbPath);
		initTestSchema(v2Db);
		try {
			expect(importMemoriesWithDb(v2Db, v2Payload)).toMatchObject({
				sessions: 1,
				user_prompts: 1,
				memory_items: 2,
				session_summaries: 1,
			});
			const memories = v2Db
				.prepare(
					`SELECT import_key, sensitivity, repository_identity, lineage_id, revision_id,
						revision_ordinal, derivation_key, source_event_ids_json, source_spans_json,
						manifest_fingerprint, provider_fingerprint, attempt_fingerprint
					 FROM memory_items ORDER BY import_key`,
				)
				.all() as Array<Record<string, unknown>>;
			expect(memories).toEqual([
				{
					attempt_fingerprint: null,
					derivation_key: null,
					import_key: "v2-invalid",
					lineage_id: null,
					manifest_fingerprint: null,
					provider_fingerprint: null,
					sensitivity: "secret",
					repository_identity: null,
					revision_id: null,
					revision_ordinal: null,
					source_event_ids_json: null,
					source_spans_json: null,
				},
				expect.objectContaining({
					import_key: "v2-private",
					sensitivity: "private",
					repository_identity: REPOSITORY_A,
					lineage_id: "d".repeat(64),
					revision_id: `revision:${"e".repeat(64)}`,
					revision_ordinal: 1,
					derivation_key: "f".repeat(64),
					source_event_ids_json: '["event-v2-private"]',
					source_spans_json: '[{"eventId":"event-v2-private","startByte":0,"endByte":18}]',
					manifest_fingerprint: CONFIGURATION_FINGERPRINT,
					provider_fingerprint: `sha256:${"b".repeat(64)}`,
					attempt_fingerprint: `sha256:${"c".repeat(64)}`,
				}),
			]);
			expect(
				v2Db
					.prepare("SELECT sensitivity, repository_identity FROM user_prompts WHERE import_key = ?")
					.get("v2-prompt"),
			).toEqual({ sensitivity: "eligible", repository_identity: REPOSITORY_A });
			expect(
				v2Db
					.prepare(
						"SELECT sensitivity, repository_identity FROM session_summaries WHERE import_key = ?",
					)
					.get("v2-summary"),
			).toEqual({ sensitivity: "local_only", repository_identity: REPOSITORY_A });
		} finally {
			v2Db.close();
		}

		const legacy = {
			...v2Payload,
			version: "1.0",
			memory_items: v2Payload.memory_items.slice(0, 1).map((row) => ({
				...row,
				import_key: "legacy-memory",
				sensitivity: "eligible",
			})),
			user_prompts: v2Payload.user_prompts.map((row) => ({
				...row,
				import_key: "legacy-prompt",
			})),
			session_summaries: v2Payload.session_summaries.map((row) => ({
				...row,
				import_key: "legacy-summary",
			})),
		} as unknown as ExportPayload;
		const legacyDbPath = createDbPath("legacy-provenance");
		const legacyDb = new Database(legacyDbPath);
		initTestSchema(legacyDb);
		try {
			importMemoriesWithDb(legacyDb, legacy, { remapProject: "forged-authority-label" });
			expect(
				legacyDb
					.prepare(
						`SELECT sensitivity, repository_identity, lineage_id, revision_id,
							source_event_ids_json, source_spans_json FROM memory_items`,
					)
					.get(),
			).toEqual({
				sensitivity: "secret",
				repository_identity: null,
				lineage_id: null,
				revision_id: null,
				source_event_ids_json: null,
				source_spans_json: null,
			});
			for (const table of ["user_prompts", "session_summaries"]) {
				expect(
					legacyDb.prepare(`SELECT sensitivity, repository_identity FROM ${table}`).get(),
				).toEqual({ sensitivity: "secret", repository_identity: null });
			}
		} finally {
			legacyDb.close();
		}
	});

	it("keeps import supersession same-repository, monotonic, and tombstone-terminal", () => {
		const dbPath = createDbPath("v2-supersession");
		const db = new Database(dbPath);
		initTestSchema(db);
		const payload = (
			importKey: string,
			repositoryIdentity: string,
			revisionOrdinal: number,
			sensitivity: "eligible" | "private",
			active = 1,
		): ExportPayload =>
			({
				version: "2.0",
				exported_at: "2026-09-01T00:00:00.000Z",
				export_metadata: {
					tool_version: "codemem",
					projects: ["display"],
					total_memories: 1,
					total_sessions: 1,
					include_inactive: true,
					filters: {},
				},
				sessions: [
					{
						id: 1,
						started_at: "2026-09-01T00:00:00.000Z",
						project: "display",
						tool_version: "test",
						import_key: `session-${repositoryIdentity}`,
						repository_identity: repositoryIdentity,
					},
				],
				memory_items: [
					{
						id: revisionOrdinal,
						session_id: 1,
						kind: "discovery",
						title: `revision ${revisionOrdinal}`,
						body_text: "same logical fact",
						created_at: `2026-09-01T00:00:0${revisionOrdinal}.000Z`,
						updated_at: `2026-09-01T00:00:0${revisionOrdinal}.000Z`,
						import_key: importKey,
						scope_id: "local-default",
						sensitivity,
						repository_identity: repositoryIdentity,
						lineage_id: "a".repeat(64),
						revision_id: `revision:${String(revisionOrdinal).repeat(64)}`,
						revision_ordinal: revisionOrdinal,
						derivation_key: "b".repeat(64),
						source_event_ids_json: ["shared-event"],
						source_spans_json: [{ eventId: "shared-event", startByte: 0, endByte: 10 }],
						manifest_fingerprint: CONFIGURATION_FINGERPRINT,
						provider_fingerprint: `sha256:${"b".repeat(64)}`,
						attempt_fingerprint: `sha256:${"c".repeat(64)}`,
						active,
						deleted_at: active === 0 ? "2026-09-01T00:00:10.000Z" : null,
					},
				],
				user_prompts: [],
				session_summaries: [],
			}) as unknown as ExportPayload;
		try {
			importMemoriesWithDb(db, payload("repo-a-r1", REPOSITORY_A, 1, "private"));
			importMemoriesWithDb(db, payload("repo-a-r2", REPOSITORY_A, 2, "eligible"));
			expect(
				db
					.prepare(
						`SELECT COUNT(*) AS active_count, MAX(sensitivity) AS sensitivity
						 FROM memory_items WHERE repository_identity = ? AND active = 1`,
					)
					.get(REPOSITORY_A),
			).toEqual({ active_count: 1, sensitivity: "private" });

			importMemoriesWithDb(db, payload("repo-b-r1", REPOSITORY_B, 1, "eligible"));
			expect(
				db
					.prepare(
						`SELECT COUNT(*) AS active_count FROM memory_items
						 WHERE repository_identity = ? AND active = 1`,
					)
					.get(REPOSITORY_B),
			).toEqual({ active_count: 1 });

			importMemoriesWithDb(db, payload("repo-a-r3-tombstone", REPOSITORY_A, 3, "eligible", 0));
			importMemoriesWithDb(db, payload("repo-a-r4-resurrection", REPOSITORY_A, 4, "eligible"));
			expect(
				db
					.prepare(
						`SELECT COUNT(*) AS active_count FROM memory_items
						 WHERE repository_identity = ? AND lineage_id = ? AND active = 1`,
					)
					.get(REPOSITORY_A, "a".repeat(64)),
			).toEqual({ active_count: 0 });
			expect(
				db
					.prepare(
						`SELECT sensitivity, deleted_at FROM memory_items
						 WHERE repository_identity = ? AND lineage_id = ? AND deleted_at IS NOT NULL
						 ORDER BY revision_ordinal DESC LIMIT 1`,
					)
					.get(REPOSITORY_A, "a".repeat(64)),
			).toEqual({ sensitivity: "private", deleted_at: "2026-09-01T00:00:10.000Z" });
		} finally {
			db.close();
		}
	});

	it("scrubs session location fields on import for every payload version", () => {
		for (const version of ["1.0", "2.0"]) {
			const payload = minimalPayload("local-default");
			(payload as { version: string }).version = version;
			const session = payload.sessions[0];
			if (!session) throw new Error("expected payload session");
			Object.assign(session as Record<string, unknown>, {
				cwd: "/restricted/import-cwd",
				git_remote: "git@example.invalid:acme/project.git",
				git_branch: "restricted-branch",
				user: "restricted-user",
			});
			const dbPath = createDbPath(`scrub-${version.replace(".", "-")}`);
			const db = new Database(dbPath);
			initTestSchema(db);
			db.close();

			importMemories(payload, { dbPath });

			const checkDb = new Database(dbPath, { readonly: true });
			try {
				// Gate fires: location fields never survive an import, whatever the
				// payload version claims. Gate passes: the display project does.
				expect(
					checkDb
						.prepare("SELECT cwd, git_remote, git_branch, user, project FROM sessions LIMIT 1")
						.get(),
				).toEqual({
					cwd: null,
					git_remote: null,
					git_branch: null,
					user: null,
					project: "codemem",
				});
			} finally {
				checkDb.close();
			}
		}
	});

	it("normalizes imported path projects and rejects a separator-only project", () => {
		const payload = minimalPayload("local-default");
		if (payload.sessions[0]) payload.sessions[0].project = "/";
		if (payload.memory_items[0]) {
			payload.memory_items[0].project = "/tmp/imported-project/";
			delete payload.memory_items[0].import_key;
		}
		const dbPath = createDbPath("project-paths");
		const db = new Database(dbPath);
		initTestSchema(db);
		db.close();

		importMemories(payload, { dbPath });

		const checkDb = new Database(dbPath, { readonly: true });
		try {
			expect(checkDb.prepare("SELECT project FROM sessions LIMIT 1").pluck().get()).toBeNull();
			expect(
				checkDb.prepare("SELECT import_key FROM memory_items LIMIT 1").pluck().get(),
			).toContain("|imported-project|");
		} finally {
			checkDb.close();
		}
	});

	it("preserves imported source scopes only when locally authorized", () => {
		const authorizedDestPath = createDbPath("authorized-import-scope");
		const authorizedDb = new Database(authorizedDestPath);
		try {
			initTestSchema(authorizedDb);
			grantScope(authorizedDb, "authorized-team");
		} finally {
			authorizedDb.close();
		}

		const result = importMemories(minimalPayload("authorized-team"), {
			dbPath: authorizedDestPath,
		});
		expect(result.memory_items).toBe(1);
		const checkDb = new Database(authorizedDestPath, { readonly: true });
		try {
			const row = checkDb.prepare("SELECT scope_id FROM memory_items LIMIT 1").get() as {
				scope_id: string;
			};
			expect(row.scope_id).toBe("authorized-team");
		} finally {
			checkDb.close();
		}

		const unauthorizedDestPath = createDbPath("unauthorized-import-scope");
		const unauthorizedDb = new Database(unauthorizedDestPath);
		try {
			initTestSchema(unauthorizedDb);
		} finally {
			unauthorizedDb.close();
		}
		expect(() =>
			importMemories(minimalPayload("authorized-team"), { dbPath: unauthorizedDestPath }),
		).toThrow(/unauthorized_scope: authorized-team/);
		expect(() =>
			importMemories(minimalPayload("legacy-shared-review"), { dbPath: unauthorizedDestPath }),
		).toThrow(/unauthorized_scope: legacy-shared-review/);
	});

	it("re-imports idempotently after a previously-authorized scope loses authorization", () => {
		// Initial import: destination has authority for the source scope.
		const destPath = createDbPath("revoked-scope-reimport");
		const grantedDb = new Database(destPath);
		try {
			initTestSchema(grantedDb);
			grantScope(grantedDb, "previously-authorized-team");
		} finally {
			grantedDb.close();
		}

		const payload = minimalPayload("previously-authorized-team");
		const initial = importMemories(payload, { dbPath: destPath });
		expect(initial.memory_items).toBe(1);

		// Revoke the scope membership/authority.
		const revokeDb = new Database(destPath);
		try {
			revokeDb
				.prepare("UPDATE replication_scopes SET status = 'archived' WHERE scope_id = ?")
				.run("previously-authorized-team");
			revokeDb
				.prepare("UPDATE scope_memberships SET status = 'revoked' WHERE scope_id = ?")
				.run("previously-authorized-team");
		} finally {
			revokeDb.close();
		}

		// Re-importing the exact same payload must be a no-op, not a hard reject.
		const second = importMemories(payload, { dbPath: destPath });
		expect(second.memory_items).toBe(0);
	});

	it("reads import payload from file", () => {
		const file = join(mkdtempSync(join(tmpdir(), "codemem-export-file-")), "export.json");
		writeFileSync(
			file,
			JSON.stringify({
				version: "1.0",
				exported_at: "2026-03-01T00:00:00Z",
				export_metadata: {},
				sessions: [],
				memory_items: [],
				session_summaries: [],
				user_prompts: [],
			}),
			"utf8",
		);

		const payload = readImportPayload(file);
		expect(payload.version).toBe("1.0");
		expect(readFileSync(file, "utf8")).toContain('"1.0"');
	});
});
