import {
	existsSync,
	mkdtempSync,
	readdirSync,
	readlinkSync,
	rmSync,
	utimesSync,
	writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import BetterSqlite3 from "better-sqlite3";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { compileDefaultCapabilityManifest } from "./capability-manifest.js";
import type { Database } from "./db.js";
import {
	assertSchemaReady,
	assertSchemaReadyReadOnly,
	backupOnFirstAccess,
	columnExists,
	connect,
	ensureAdditiveSchemaCompatibility,
	ensurePlannerStats,
	fromJson,
	getSchemaVersion,
	isEmbeddingDisabled,
	loadSqliteVec,
	rawEventPayloadDigest,
	resolveDbPath,
	SCHEMA_VERSION,
	tableExists,
	toJson,
} from "./db.js";
import { compileProviderDestinationBoundary } from "./destination-boundary.js";
import { runDatabaseMigrations } from "./migration-runner.js";
import { bootstrapSchema } from "./schema-bootstrap.js";
import { resolveStorageLayout, runLegacyMigration, sha256File } from "./storage.js";
import { MemoryStore } from "./store.js";

const verifyTestBackup = () => ({ verified: true, evidence: "db-test-backup" });

function connectMigrated(path: string): Database {
	const db = connect(path);
	try {
		runDatabaseMigrations(db, { dbPath: path, backupAndVerify: verifyTestBackup });
		return db;
	} catch (error) {
		db.close();
		throw error;
	}
}

function connectBootstrapped(path: string): Database {
	const db = connect(path);
	bootstrapSchema(db);
	return db;
}

function hasIndex(db: Database, name: string): boolean {
	const row = db
		.prepare("SELECT 1 AS ok FROM sqlite_master WHERE type = 'index' AND name = ? LIMIT 1")
		.get(name) as { ok: number } | undefined;
	return row?.ok === 1;
}

function columnInfo(
	db: Database,
	table: string,
	column: string,
): { is_not_null: number } | undefined {
	return db
		.prepare('SELECT "notnull" AS is_not_null FROM pragma_table_info(?) WHERE name = ? LIMIT 1')
		.get(table, column) as { is_not_null: number } | undefined;
}

const SLICE1_V21_TABLES = [
	"raw_event_identity_conflicts",
	"raw_event_quarantine",
	"processing_resume_producer_receipts",
	"processing_resume_signals",
	"provider_health_states",
] as const;

const SLICE1_V21_COLUMNS = [
	[
		"raw_events",
		[
			"sensitivity",
			"repository_identity",
			"capture_manifest_fingerprint",
			"capture_state",
			"safe_error_code",
			"payload_digest_version",
			"payload_digest",
		],
	],
	[
		"raw_event_flush_batches",
		[
			"admission_manifest_fingerprint",
			"admission_provider_fingerprint",
			"retry_limit",
			"claim_generation",
			"attempt_manifest_fingerprint",
			"attempt_provider_fingerprint",
			"attempt_fingerprint",
			"attempt_max_memory_items",
			"resume_grant_state",
			"last_resume_sequence",
			"egress_diagnostic_json",
			"completion_disposition",
			"legacy_recovery_state",
			"frontier_already_advanced",
		],
	],
	[
		"memory_items",
		[
			"sensitivity",
			"repository_identity",
			"lineage_id",
			"revision_id",
			"revision_ordinal",
			"supersedes_memory_id",
			"derivation_key",
			"source_event_ids_json",
			"source_spans_json",
			"manifest_fingerprint",
			"provider_fingerprint",
			"attempt_fingerprint",
		],
	],
	["user_prompts", ["sensitivity", "repository_identity"]],
	["session_summaries", ["sensitivity", "repository_identity"]],
	["artifacts", ["sensitivity", "repository_identity"]],
	["sessions", ["repository_identity"]],
	["processing_resume_producer_receipts", ["target_job_ids_json"]],
] as const;

function slice1ColumnShape(db: Database) {
	return SLICE1_V21_COLUMNS.flatMap(([table, columns]) =>
		columns.map((column) => ({
			table,
			column,
			...(db
				.prepare(
					`SELECT type, "notnull" AS not_null, dflt_value AS default_value
					 FROM pragma_table_info(?) WHERE name = ?`,
				)
				.get(table, column) as {
				type: string;
				not_null: number;
				default_value: string | null;
			}),
		})),
	);
}

function tableDefinition(db: Database, table: string): string {
	const row = db
		.prepare("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?")
		.get(table) as { sql: string | null } | undefined;
	return row?.sql ?? "";
}

function tableDdl(db: Database, table: string): string {
	return (
		db
			.prepare(
				"SELECT sql FROM sqlite_master WHERE (type = 'table' AND name = ?) OR (type = 'index' AND tbl_name = ?) ORDER BY type, name",
			)
			.all(table, table) as Array<{ sql: string | null }>
	)
		.map((row) => row.sql ?? "")
		.join("\n");
}

function normalizedSlice1Ddl(db: Database): string[] {
	return ["raw_events", "raw_event_flush_batches", ...SLICE1_V21_TABLES]
		.flatMap((table) => tableDdl(db, table).split("\n"))
		.map((ddl) => ddl.replace(/\s+/g, " ").trim())
		.filter(Boolean)
		.sort();
}

function seedV20Slice1Database(path: string): Database {
	const db = connect(path);
	db.exec(`
		CREATE TABLE sessions (id INTEGER PRIMARY KEY, started_at TEXT NOT NULL, project TEXT);
		CREATE TABLE memory_items (
			id INTEGER PRIMARY KEY, session_id INTEGER, kind TEXT NOT NULL, title TEXT NOT NULL,
			body_text TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1,
			created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
			import_key TEXT, origin_device_id TEXT
		);
		CREATE TABLE artifacts (id INTEGER PRIMARY KEY, session_id INTEGER, content TEXT NOT NULL);
		CREATE TABLE usage_events (id INTEGER PRIMARY KEY);
		CREATE VIRTUAL TABLE memory_fts USING fts5(title);
		CREATE TABLE raw_events (
			id INTEGER PRIMARY KEY, source TEXT NOT NULL DEFAULT 'opencode', stream_id TEXT NOT NULL DEFAULT '',
			opencode_session_id TEXT NOT NULL, event_id TEXT, event_seq INTEGER NOT NULL,
			event_type TEXT NOT NULL, ts_wall_ms INTEGER, ts_mono_ms REAL,
			payload_json TEXT NOT NULL, created_at TEXT NOT NULL
		);
		CREATE UNIQUE INDEX idx_raw_events_source_stream_seq ON raw_events(source, stream_id, event_seq);
		CREATE UNIQUE INDEX idx_raw_events_source_stream_event_id ON raw_events(source, stream_id, event_id);
		CREATE TABLE raw_event_sessions (
			source TEXT NOT NULL DEFAULT 'opencode', stream_id TEXT NOT NULL DEFAULT '',
			opencode_session_id TEXT NOT NULL, cwd TEXT, project TEXT, started_at TEXT,
			last_seen_ts_wall_ms INTEGER, last_received_event_seq INTEGER NOT NULL DEFAULT -1,
			last_flushed_event_seq INTEGER NOT NULL DEFAULT -1, updated_at TEXT NOT NULL,
			PRIMARY KEY(source, stream_id)
		);
		CREATE TABLE raw_event_flush_batches (
			id INTEGER PRIMARY KEY, source TEXT NOT NULL DEFAULT 'opencode', stream_id TEXT NOT NULL DEFAULT '',
			opencode_session_id TEXT NOT NULL, start_event_seq INTEGER NOT NULL, end_event_seq INTEGER NOT NULL,
			extractor_version TEXT NOT NULL, status TEXT NOT NULL, attempt_count INTEGER NOT NULL DEFAULT 0,
			created_at TEXT NOT NULL, updated_at TEXT NOT NULL
		);
		CREATE TABLE user_prompts (
			id INTEGER PRIMARY KEY, session_id INTEGER, project TEXT, prompt_text TEXT NOT NULL,
			created_at TEXT NOT NULL, created_at_epoch INTEGER NOT NULL
		);
		CREATE TABLE session_summaries (
			id INTEGER PRIMARY KEY, session_id INTEGER, project TEXT, request TEXT,
			created_at TEXT NOT NULL, created_at_epoch INTEGER NOT NULL
		);
	`);

	const now = "2026-08-31T00:00:00.000Z";
	db.prepare("INSERT INTO sessions(id, started_at, project) VALUES (1, ?, 'legacy')").run(now);
	db.prepare(
		"INSERT INTO memory_items(id, session_id, kind, title, body_text, created_at, updated_at) VALUES (1, 1, 'summary', 'legacy', 'legacy', ?, ?)",
	).run(now, now);
	db.prepare("INSERT INTO artifacts(id, session_id, content) VALUES (1, 1, 'legacy')").run();
	db.prepare(
		"INSERT INTO user_prompts(id, session_id, prompt_text, created_at, created_at_epoch) VALUES (1, 1, 'legacy', ?, 0)",
	).run(now);
	db.prepare(
		"INSERT INTO session_summaries(id, session_id, request, created_at, created_at_epoch) VALUES (1, 1, 'legacy', ?, 0)",
	).run(now);

	const addStream = (stream: string, frontier: number) => {
		db.prepare(
			"INSERT INTO raw_event_sessions(source, stream_id, opencode_session_id, last_flushed_event_seq, updated_at) VALUES ('opencode', ?, ?, ?, ?)",
		).run(stream, stream, frontier, now);
	};
	const addEvent = (stream: string, sequence: number) => {
		db.prepare(
			"INSERT INTO raw_events(source, stream_id, opencode_session_id, event_id, event_seq, event_type, payload_json, created_at) VALUES ('opencode', ?, ?, ?, ?, 'message', '{}', ?)",
		).run(stream, stream, `${stream}-${sequence}`, sequence, now);
	};
	const addGaveUp = (id: number, stream: string, start: number, end: number) => {
		db.prepare(
			"INSERT INTO raw_event_flush_batches(id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq, extractor_version, status, created_at, updated_at) VALUES (?, 'opencode', ?, ?, ?, ?, 'raw_events_v1', 'gave_up', ?, ?)",
		).run(id, stream, stream, start, end, now, now);
	};

	addStream("complete", 2);
	addEvent("complete", 1);
	addEvent("complete", 2);
	addGaveUp(1, "complete", 1, 2);
	addStream("missing", 2);
	addEvent("missing", 1);
	addGaveUp(2, "missing", 1, 2);
	addStream("overlap", 3);
	addEvent("overlap", 1);
	addEvent("overlap", 2);
	addEvent("overlap", 3);
	addGaveUp(3, "overlap", 1, 2);
	addGaveUp(4, "overlap", 2, 3);
	addStream("pending", -1);
	addEvent("pending", 0);
	db.prepare(
		"INSERT INTO raw_event_flush_batches(id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq, extractor_version, status, created_at, updated_at) VALUES (5, 'opencode', 'pending', 'pending', 0, 0, 'raw_events_v1', 'pending', ?, ?)",
	).run(now, now);
	addStream("invalid-json", -1);
	db.prepare(
		"INSERT INTO raw_events(source, stream_id, opencode_session_id, event_id, event_seq, event_type, payload_json, created_at) VALUES ('opencode', 'invalid-json', 'invalid-json', 'invalid-json-0', 0, 'message', 'not-json', ?)",
	).run(now);
	db.pragma("user_version = 20");
	return db;
}

describe("connect", () => {
	let tmpDir: string;
	let db: Database | undefined;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("opens a database with WAL mode", () => {
		db = connectMigrated(join(tmpDir, "test.sqlite"));
		const mode = db.pragma("journal_mode", { simple: true }) as string;
		expect(mode.toLowerCase()).toBe("wal");
	});

	it("sets busy_timeout to 5000ms", () => {
		db = connectMigrated(join(tmpDir, "test.sqlite"));
		const timeout = db.pragma("busy_timeout", { simple: true });
		expect(timeout).toBe(5000);
	});

	it("enables foreign keys", () => {
		db = connectMigrated(join(tmpDir, "test.sqlite"));
		const fk = db.pragma("foreign_keys", { simple: true });
		expect(fk).toBe(1);
	});

	it("sets synchronous to NORMAL", () => {
		db = connectMigrated(join(tmpDir, "test.sqlite"));
		const sync = db.pragma("synchronous", { simple: true });
		// NORMAL = 1
		expect(sync).toBe(1);
	});

	it("applies read-tuning pragmas (cache_size, mmap_size, temp_store)", () => {
		db = connectMigrated(join(tmpDir, "test.sqlite"));
		expect(db.pragma("cache_size", { simple: true })).toBe(-65536);
		// mmap_size readback is clamped to the build's SQLITE_MAX_MMAP_SIZE, so
		// assert it's enabled (>0) rather than an exact, build-dependent value.
		expect(db.pragma("mmap_size", { simple: true })).toBeGreaterThan(0);
		// temp_store: 2 = MEMORY
		expect(db.pragma("temp_store", { simple: true })).toBe(2);
	});

	it("drops legacy memory_items indexes via additive compatibility", () => {
		db = connectBootstrapped(join(tmpDir, "legacy-idx.sqlite"));
		// Simulate a database created by an older schema that carried the
		// now-obsolete indexes the current schema never creates.
		db.exec(
			`CREATE INDEX IF NOT EXISTS idx_memory_items_visibility ON memory_items(visibility);
			 CREATE INDEX IF NOT EXISTS idx_memory_items_workspace_kind ON memory_items(workspace_kind);
			 CREATE INDEX IF NOT EXISTS idx_memory_items_user_prompt_id ON memory_items(user_prompt_id);`,
		);
		// Drop back to a legacy user_version so the additive shim actually runs
		// (a fresh user_version=SCHEMA_VERSION DB short-circuits the gated DDL,
		// and never carries these legacy indexes in the first place).
		db.pragma("user_version = 6");
		expect(hasIndex(db, "idx_memory_items_visibility")).toBe(true);

		ensureAdditiveSchemaCompatibility(db);

		expect(hasIndex(db, "idx_memory_items_visibility")).toBe(false);
		expect(hasIndex(db, "idx_memory_items_workspace_kind")).toBe(false);
		expect(hasIndex(db, "idx_memory_items_user_prompt_id")).toBe(false);
		// A composite index that legitimately covers visibility still exists.
		expect(hasIndex(db, "idx_memory_items_scope_visibility_created")).toBe(true);
	});

	it("creates parent directories if they don't exist", () => {
		const nested = join(tmpDir, "deep", "nested", "dir", "test.sqlite");
		db = connectMigrated(nested);
		expect(db.pragma("journal_mode", { simple: true })).toBe("wal");
	});

	it("expands ~/ paths like Python", () => {
		expect(resolveDbPath("~/codemem-test.sqlite")).toBe(join(homedir(), "codemem-test.sqlite"));
	});

	it("bootstraps the schema on a fresh database path", () => {
		db = connectMigrated(join(tmpDir, "fresh.sqlite"));

		expect(getSchemaVersion(db)).toBe(SCHEMA_VERSION);
		expect(tableExists(db, "memory_items")).toBe(true);
		expect(tableExists(db, "sessions")).toBe(true);
		expect(tableExists(db, "replication_scopes")).toBe(true);
		expect(tableExists(db, "project_scope_mappings")).toBe(true);
		expect(tableExists(db, "scope_memberships")).toBe(true);
		expect(tableExists(db, "sync_reset_state_v2")).toBe(true);
		expect(tableExists(db, "sync_retention_state_v2")).toBe(true);
		expect(tableExists(db, "replication_cursors_v2")).toBe(true);
		expect(tableExists(db, "recipient_policy_authority_states")).toBe(true);
		expect(tableExists(db, "recipient_policy_reconciliation_steps")).toBe(true);
		expect(tableExists(db, "recipient_policy_deny_overlays")).toBe(true);
		expect(tableExists(db, "coordinator_enrollment_reconciliation_issues")).toBe(true);
		expect(tableExists(db, "recipient_managed_project_projections")).toBe(true);
		expect(hasIndex(db, "idx_recipient_managed_projects_identity_status")).toBe(true);
		expect(hasIndex(db, "idx_recipient_managed_projects_scope_authority")).toBe(true);
		expect(columnExists(db, "memory_items", "scope_id")).toBe(true);
		expect(columnExists(db, "replication_ops", "scope_id")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_origin_device_active")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_scope_visibility_created")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_scope_backfill_pending")).toBe(true);
		expect(hasIndex(db, "idx_replication_ops_scope_created")).toBe(true);
		expect(hasIndex(db, "idx_replication_cursors_v2_scope")).toBe(true);
		expect(hasIndex(db, "idx_coordinator_enrollment_issues_boundary_status")).toBe(true);
		expect(hasIndex(db, "idx_coordinator_enrollment_issues_status_recent")).toBe(true);
		expect(hasIndex(db, "idx_recipient_policy_reconciliation_steps_pending_refresh")).toBe(true);
		expect(() => assertSchemaReady(db)).not.toThrow();
	});

	it("bootstraps the schema on an empty existing database file", () => {
		const dbPath = join(tmpDir, "empty.sqlite");
		writeFileSync(dbPath, "");

		db = connectMigrated(dbPath);

		expect(getSchemaVersion(db)).toBe(SCHEMA_VERSION);
		expect(tableExists(db, "memory_items")).toBe(true);
		expect(() => assertSchemaReady(db)).not.toThrow();
	});

	it("reopens an initialized database without clobbering existing data", () => {
		const dbPath = join(tmpDir, "reopen.sqlite");
		db = connectMigrated(dbPath);
		db.exec("CREATE TABLE connect_reopen_guard (id INTEGER PRIMARY KEY, label TEXT NOT NULL)");
		db.prepare("INSERT INTO connect_reopen_guard(label) VALUES (?)").run("still here");
		db.close();

		db = connectMigrated(dbPath);

		expect(getSchemaVersion(db)).toBe(SCHEMA_VERSION);
		expect(
			db.prepare("SELECT label FROM connect_reopen_guard WHERE id = 1").get() as
				| { label: string }
				| undefined,
		).toEqual({ label: "still here" });
	});

	it("supports multiple handles racing the first-run bootstrap result", () => {
		const dbPath = join(tmpDir, "multi-handle.sqlite");
		const first = connectMigrated(dbPath);
		const second = connectMigrated(dbPath);
		try {
			expect(getSchemaVersion(first)).toBe(SCHEMA_VERSION);
			expect(getSchemaVersion(second)).toBe(SCHEMA_VERSION);
			expect(tableExists(first, "memory_items")).toBe(true);
			expect(tableExists(second, "memory_items")).toBe(true);
			expect(() => assertSchemaReady(first)).not.toThrow();
			expect(() => assertSchemaReady(second)).not.toThrow();
		} finally {
			first.close();
			second.close();
		}
	});

	it("does not bootstrap or switch unrelated non-empty databases to WAL", () => {
		const dbPath = join(tmpDir, "unrelated.sqlite");
		const unrelated = new BetterSqlite3(dbPath);
		unrelated.exec("CREATE TABLE unrelated_data (id INTEGER PRIMARY KEY)");
		unrelated.close();

		db = connect(dbPath);

		expect(getSchemaVersion(db)).toBe(0);
		expect(tableExists(db, "memory_items")).toBe(false);
		expect(tableExists(db, "unrelated_data")).toBe(true);
		expect(existsSync(`${dbPath}-wal`)).toBe(false);
	});

	it("does not switch unrelated databases with nonzero user_version to WAL", () => {
		const dbPath = join(tmpDir, "unrelated-versioned.sqlite");
		const unrelated = new BetterSqlite3(dbPath);
		unrelated.exec("CREATE TABLE unrelated_data (id INTEGER PRIMARY KEY)");
		unrelated.pragma("user_version = 1");
		unrelated.close();

		db = connect(dbPath);

		expect(getSchemaVersion(db)).toBe(1);
		expect(tableExists(db, "memory_items")).toBe(false);
		expect(tableExists(db, "unrelated_data")).toBe(true);
		expect(existsSync(`${dbPath}-wal`)).toBe(false);
	});
});

describe("schema v21 Slice 1 persistence", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-schema-v21-"));
	});

	afterEach(() => {
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("creates the closed v21 tables, columns, checks, and raw-event identity index", () => {
		const fresh = connectMigrated(join(tmpDir, "fresh.sqlite"));
		try {
			expect(SCHEMA_VERSION).toBe(21);
			for (const table of SLICE1_V21_TABLES) expect(tableExists(fresh, table)).toBe(true);
			for (const [table, columns] of SLICE1_V21_COLUMNS) {
				for (const column of columns) expect(columnExists(fresh, table, column)).toBe(true);
			}

			expect(tableDefinition(fresh, "raw_events")).toMatch(/CHECK[\s\S]*sensitivity/i);
			expect(tableDefinition(fresh, "raw_event_flush_batches")).toContain("retry_exhausted");
			const rawEventDdl = tableDdl(fresh, "raw_events");
			expect(rawEventDdl).not.toContain("idx_raw_events_source_stream_event_id");
			expect(rawEventDdl).toMatch(
				/CREATE UNIQUE INDEX[\s\S]*COALESCE\([^)]*repository_identity[^)]*repo-v1:unknown[^)]*\)[\s\S]*source[\s\S]*stream_id[\s\S]*event_id/i,
			);

			const conflictDdl = tableDdl(fresh, "raw_event_identity_conflicts");
			expect(conflictDdl).toMatch(/canonical.*digest/i);
			expect(conflictDdl).toMatch(/conflicting.*digest/i);
			expect(conflictDdl).toMatch(/UNIQUE/i);
			const quarantineDdl = tableDdl(fresh, "raw_event_quarantine");
			expect(quarantineDdl).toMatch(/receipt/i);
			expect(quarantineDdl).toMatch(/UNIQUE/i);
			expect(quarantineDdl).toContain("repo-v1:unknown");
			expect(conflictDdl).toContain("repo-v1:unknown");
			expect(tableDdl(fresh, "processing_resume_producer_receipts")).toMatch(
				/receipt_id[\s\S]*PRIMARY KEY/i,
			);
			expect(tableDdl(fresh, "processing_resume_signals")).toMatch(/job_id/i);
			const sessionId = Number(
				fresh.prepare("INSERT INTO sessions(started_at) VALUES ('2026-08-31T00:00:00.000Z')").run()
					.lastInsertRowid,
			);
			const memoryId = Number(
				fresh
					.prepare(
						"INSERT INTO memory_items(session_id, kind, title, body_text, created_at, updated_at) VALUES (?, 'discovery', 'title', 'body', '2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')",
					)
					.run(sessionId).lastInsertRowid,
			);
			expect(() =>
				fresh.prepare("UPDATE memory_items SET sensitivity = 'invalid' WHERE id = ?").run(memoryId),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						"INSERT INTO raw_events(source, stream_id, opencode_session_id, event_id, event_seq, event_type, payload_json, created_at, payload_digest) VALUES ('opencode', 'invalid', 'invalid', NULL, 0, 'message', '{}', '2026-08-31T00:00:00.000Z', ?)",
					)
					.run(rawEventPayloadDigest({})),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						"INSERT INTO raw_events(source, stream_id, opencode_session_id, event_id, event_seq, event_type, payload_json, created_at, capture_state, payload_digest) VALUES ('opencode', 'quarantine-check', 'quarantine-check', 'quarantine-check', 0, 'message', '{}', '2026-08-31T00:00:00.000Z', 'quarantined', ?)",
					)
					.run(rawEventPayloadDigest({})),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_event_quarantine(
							receipt_id, source, stream_id, event_id, event_type, payload_digest_version,
							payload_digest, safe_error_code, first_seen_at, last_seen_at
						 ) VALUES ('invalid-code', 'opencode', 'invalid-code', 'invalid-code', 'message',
							'event-payload-digest-v1', ?, 'invalid_code',
							'2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')`,
					)
					.run(`sha256:${"0".repeat(64)}`),
			).toThrow();
			const validDigest = rawEventPayloadDigest({});
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_event_identity_conflicts(
							receipt_id, source, stream_id, event_id, payload_digest_version,
							canonical_payload_digest, conflicting_payload_digest, first_seen_at, last_seen_at
						 ) VALUES ('invalid-conflict-digest', 'opencode', 'invalid', 'invalid',
							'wrong-version', ?, ?, '2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')`,
					)
					.run(`sha256:${"a".repeat(64)}`, `sha256:${"b".repeat(64)}`),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO provider_health_states(
							configuration_fingerprint, provider_fingerprint, health_state, updated_at
						 ) VALUES (?, ?, 'unhealthy', '2026-08-31T00:00:00.000Z')`,
					)
					.run(`sha256:${"a".repeat(64)}`, `sha256:${"b".repeat(64)}`),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_event_quarantine(
							receipt_id, source, stream_id, event_id, event_type, payload_digest_version,
							payload_digest, safe_error_code, first_seen_at, last_seen_at
						 ) VALUES ('invalid-quarantine-digest', 'opencode', 'invalid', 'invalid', 'message',
							'wrong-version', '', 'repository_identity_unknown_collision',
							'2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')`,
					)
					.run(),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_events(
							source, stream_id, opencode_session_id, event_id, event_seq, event_type,
							payload_json, created_at, repository_identity, payload_digest
						 ) VALUES ('opencode', 'sentinel', 'sentinel', 'sentinel', 0, 'message', '{}',
							'2026-08-31T00:00:00.000Z', 'repo-v1:unknown', ?)`,
					)
					.run(validDigest),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_events(
							source, stream_id, opencode_session_id, event_id, event_seq, event_type,
							payload_json, created_at, payload_digest
						 ) VALUES ('opencode', 'zero-digest', 'zero-digest', 'zero-digest', 0, 'message', '{}',
							'2026-08-31T00:00:00.000Z', ?)`,
					)
					.run(`sha256:${"0".repeat(64)}`),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_event_identity_conflicts(
							receipt_id, repository_identity, source, stream_id, event_id,
							payload_digest_version, canonical_payload_digest, conflicting_payload_digest,
							first_seen_at, last_seen_at
						 ) VALUES ('sentinel-conflict', 'repo-v1:unknown', 'opencode', 'sentinel', 'sentinel',
							'event-payload-digest-v1', ?, ?,
							'2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')`,
					)
					.run(`sha256:${"a".repeat(64)}`, `sha256:${"b".repeat(64)}`),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_event_quarantine(
							receipt_id, repository_identity, source, stream_id, event_id, event_type,
							payload_digest_version, payload_digest, safe_error_code, first_seen_at, last_seen_at
						 ) VALUES ('sentinel-quarantine', 'repo-v1:unknown', 'opencode', 'sentinel',
							'sentinel', 'message', 'event-payload-digest-v1', ?,
							'repository_identity_unknown_collision',
							'2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')`,
					)
					.run(validDigest),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO provider_health_states(
							configuration_fingerprint, provider_fingerprint, health_state, updated_at
						 ) VALUES (?, ?, 'unknown', '2026-08-31T00:00:00.000Z')`,
					)
					.run(`sha256:${"a".repeat(64)}`, `sha256:${"b".repeat(64)}`),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO provider_health_states(
							configuration_fingerprint, provider_fingerprint, health_state,
							safe_error_code, updated_at
						 ) VALUES (?, ?, 'healthy', 'arbitrary', '2026-08-31T00:00:00.000Z')`,
					)
					.run(`sha256:${"a".repeat(64)}`, `sha256:${"b".repeat(64)}`),
			).toThrow();
			fresh
				.prepare(
					`INSERT INTO processing_resume_producer_receipts(
						receipt_id, producer_kind, configuration_fingerprint, provider_fingerprint,
						producer_sequence, created_at
					 ) VALUES ('valid-receipt', 'user_confirmed_doctor_retry', ?, ?, 1,
						'2026-08-31T00:00:00.000Z')`,
				)
				.run(`sha256:${"a".repeat(64)}`, `sha256:${"b".repeat(64)}`);
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO processing_resume_signals(
							signal_id, job_id, producer_receipt_id, sequence, target_provider_fingerprint,
							target_manifest_fingerprint, kind, disposition, created_at
						 ) VALUES ('orphan-signal', 999, 'missing-receipt', 1, ?, ?,
							'user_confirmed_doctor_retry', 'wrong_job', '2026-08-31T00:00:00.000Z')`,
					)
					.run(`sha256:${"b".repeat(64)}`, `sha256:${"a".repeat(64)}`),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO processing_resume_signals(
							signal_id, job_id, producer_receipt_id, sequence, kind, disposition, created_at
						 ) VALUES ('missing-targets', 999, 'valid-receipt', 1,
							'user_confirmed_doctor_retry', 'wrong_job', '2026-08-31T00:00:00.000Z')`,
					)
					.run(),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_event_flush_batches(
							source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
							extractor_version, status, last_resume_signal_disposition, created_at, updated_at
						 ) VALUES ('opencode', 'invalid-disposition', 'invalid-disposition', 0, 0,
							'raw_events_v1', 'queued', 'not_closed',
							'2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')`,
					)
					.run(),
			).toThrow();
			expect(() =>
				fresh
					.prepare(
						`INSERT INTO raw_event_flush_batches(
							source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
							extractor_version, status, resume_grant_reason, created_at, updated_at
						 ) VALUES ('opencode', 'invalid-grant-reason', 'invalid-grant-reason', 0, 0,
							'raw_events_v1', 'queued', 'not_closed',
							'2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z')`,
					)
					.run(),
			).toThrow();
		} finally {
			fresh.close();
		}
	});

	it("migrates v20 atomically after backup, preserves conservative provenance, and audits legacy gave_up ranges", () => {
		expect(SCHEMA_VERSION).toBe(21);
		const refused = seedV20Slice1Database(join(tmpDir, "refused.sqlite"));
		const beforeRefusal = normalizedSlice1Ddl(refused);
		try {
			const refusedBackup = vi.fn(() => ({ verified: false, evidence: "" }));
			expect(() =>
				runDatabaseMigrations(refused, {
					dbPath: refused.name,
					backupAndVerify: refusedBackup,
				}),
			).toThrow(/verified backup/i);
			expect(refusedBackup).toHaveBeenCalledOnce();
			expect(getSchemaVersion(refused)).toBe(20);
			expect(normalizedSlice1Ddl(refused)).toEqual(beforeRefusal);
		} finally {
			refused.close();
		}

		let migrated: Database | undefined;
		let reopened: Database | undefined;
		let fresh: Database | undefined;
		try {
			migrated = seedV20Slice1Database(join(tmpDir, "migrated.sqlite"));
			migrated.exec(`
				INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				) VALUES
					('opencode', 'completed-frontier-gap', 'completed-frontier-gap', 2, -1,
						'2026-08-31T00:00:00.000Z'),
					('opencode', 'completed-already-advanced', 'completed-already-advanced', 0, 0,
						'2026-08-31T00:00:00.000Z');
				INSERT INTO raw_events(
					source, stream_id, opencode_session_id, event_id, event_seq, event_type,
					payload_json, created_at
				) VALUES
					('opencode', 'completed-frontier-gap', 'completed-frontier-gap',
						'completed-frontier-gap-0', 0, 'message', '{}', '2026-08-31T00:00:00.000Z'),
					('opencode', 'completed-frontier-gap', 'completed-frontier-gap',
						'completed-frontier-gap-1', 1, 'message', '{}', '2026-08-31T00:00:00.000Z'),
					('opencode', 'completed-frontier-gap', 'completed-frontier-gap',
						'completed-frontier-gap-2', 2, 'message', '{}', '2026-08-31T00:00:00.000Z'),
					('opencode', 'completed-already-advanced', 'completed-already-advanced',
						'completed-already-advanced-0', 0, 'message', '{}',
						'2026-08-31T00:00:00.000Z');
				INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, created_at, updated_at
				) VALUES
					(6, 'opencode', 'completed-frontier-gap', 'completed-frontier-gap', 0, 0,
						'raw_events_v1', 'completed', '2026-08-31T00:00:00.000Z',
						'2026-08-31T00:00:00.000Z'),
					(7, 'opencode', 'completed-already-advanced', 'completed-already-advanced', 0, 0,
						'raw_events_v1', 'completed', '2026-08-31T00:00:00.000Z',
						'2026-08-31T00:00:00.000Z'),
					(8, 'opencode', 'completed-frontier-gap', 'completed-frontier-gap', 1, 1,
						'raw_events_v1', 'completed', '2026-08-31T00:00:00.000Z',
						'2026-08-31T00:00:00.000Z');
				CREATE TABLE schema_compat_state (
					id INTEGER PRIMARY KEY, applied_schema_version INTEGER NOT NULL, applied_at TEXT NOT NULL
				);
				INSERT INTO schema_compat_state VALUES (1, 21, '2026-08-31T00:00:00.000Z');
				ALTER TABLE raw_event_flush_batches ADD COLUMN error_message TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN error_type TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN observer_provider TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN observer_model TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN observer_runtime TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN observer_auth_source TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN observer_auth_type TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN observer_error_code TEXT;
				ALTER TABLE raw_event_flush_batches ADD COLUMN observer_error_message TEXT;
				UPDATE raw_event_flush_batches SET
					error_message = 'legacy-message', error_type = 'legacy-type',
					observer_provider = 'legacy-provider', observer_model = 'legacy-model',
					observer_runtime = 'legacy-runtime', observer_auth_source = 'legacy-auth-source',
					observer_auth_type = 'legacy-auth-type', observer_error_code = 'legacy-code',
					observer_error_message = 'legacy-observer-message'
				WHERE id = 1;
			`);
			const verifiedBackup = vi.fn(verifyTestBackup);
			runDatabaseMigrations(migrated, {
				dbPath: migrated.name,
				backupAndVerify: verifiedBackup,
			});
			expect(verifiedBackup).toHaveBeenCalledExactlyOnceWith(
				expect.objectContaining({ kind: "upgrade", schemaVersion: 20 }),
			);
			expect(getSchemaVersion(migrated)).toBe(21);
			for (const column of [
				"sensitivity",
				"repository_identity",
				"capture_manifest_fingerprint",
				"payload_digest_version",
				"payload_digest",
			]) {
				expect(columnExists(migrated, "raw_events", column)).toBe(true);
			}

			const legacyRaw = migrated
				.prepare(
					"SELECT sensitivity, repository_identity, capture_manifest_fingerprint, payload_digest_version, payload_digest FROM raw_events WHERE stream_id = 'complete' AND event_seq = 1",
				)
				.get() as {
				sensitivity: string;
				repository_identity: string | null;
				capture_manifest_fingerprint: string | null;
				payload_digest_version: string;
				payload_digest: string;
			};
			expect(legacyRaw).toMatchObject({
				sensitivity: "secret",
				repository_identity: null,
				capture_manifest_fingerprint: null,
				payload_digest_version: "event-payload-digest-v1",
			});
			expect(legacyRaw.payload_digest).toBe(rawEventPayloadDigest({}));
			expect(
				migrated
					.prepare(
						"SELECT payload_json, sensitivity, capture_state, safe_error_code FROM raw_events WHERE event_id = 'invalid-json-0'",
					)
					.get(),
			).toEqual({
				payload_json: "{}",
				sensitivity: "secret",
				capture_state: "quarantined",
				safe_error_code: "redaction_degraded",
			});
			expect(
				migrated
					.prepare(
						"SELECT COUNT(*) AS count FROM raw_event_quarantine WHERE event_id = 'invalid-json-0'",
					)
					.get(),
			).toEqual({ count: 1 });
			for (const table of ["memory_items", "user_prompts", "session_summaries", "artifacts"]) {
				expect(columnExists(migrated, table, "sensitivity")).toBe(true);
				expect(columnExists(migrated, table, "repository_identity")).toBe(true);
				expect(
					migrated
						.prepare(`SELECT sensitivity, repository_identity FROM ${table} WHERE id = 1`)
						.get(),
				).toEqual({ sensitivity: "secret", repository_identity: null });
			}
			expect(columnExists(migrated, "sessions", "repository_identity")).toBe(true);
			expect(
				migrated.prepare("SELECT repository_identity FROM sessions WHERE id = 1").get(),
			).toEqual({ repository_identity: null });

			for (const column of [
				"status",
				"completion_disposition",
				"legacy_recovery_state",
				"frontier_already_advanced",
			]) {
				expect(columnExists(migrated, "raw_event_flush_batches", column)).toBe(true);
			}
			expect(
				migrated
					.prepare(
						"SELECT id, status, completion_disposition, legacy_recovery_state, frontier_already_advanced FROM raw_event_flush_batches ORDER BY id",
					)
					.all(),
			).toEqual([
				{
					id: 1,
					status: "retry_exhausted",
					completion_disposition: "none",
					legacy_recovery_state: "complete_range",
					frontier_already_advanced: 1,
				},
				{
					id: 2,
					status: "completed",
					completion_disposition: "legacy_unrecoverable",
					legacy_recovery_state: "missing_or_ambiguous_range",
					frontier_already_advanced: 0,
				},
				{
					id: 3,
					status: "completed",
					completion_disposition: "legacy_unrecoverable",
					legacy_recovery_state: "missing_or_ambiguous_range",
					frontier_already_advanced: 0,
				},
				{
					id: 4,
					status: "completed",
					completion_disposition: "legacy_unrecoverable",
					legacy_recovery_state: "missing_or_ambiguous_range",
					frontier_already_advanced: 0,
				},
				{
					id: 5,
					status: "retry_exhausted",
					completion_disposition: "none",
					legacy_recovery_state: "complete_range",
					frontier_already_advanced: 0,
				},
				{
					id: 6,
					status: "completed",
					completion_disposition: "none",
					legacy_recovery_state: "not_legacy",
					frontier_already_advanced: 0,
				},
				{
					id: 7,
					status: "completed",
					completion_disposition: "none",
					legacy_recovery_state: "not_legacy",
					frontier_already_advanced: 0,
				},
				{
					id: 8,
					status: "completed",
					completion_disposition: "none",
					legacy_recovery_state: "not_legacy",
					frontier_already_advanced: 0,
				},
			]);
			expect(
				migrated
					.prepare(
						`SELECT id, attempt_count, admission_manifest_fingerprint,
							admission_provider_fingerprint, attempt_manifest_fingerprint,
							attempt_provider_fingerprint, attempt_fingerprint
						 FROM raw_event_flush_batches WHERE id IN (1, 5) ORDER BY id`,
					)
					.all(),
			).toEqual(
				[1, 5].map((id) => ({
					id,
					attempt_count: 0,
					admission_manifest_fingerprint: null,
					admission_provider_fingerprint: null,
					attempt_manifest_fingerprint: null,
					attempt_provider_fingerprint: null,
					attempt_fingerprint: null,
				})),
			);
			const migratedStore = new MemoryStore(migrated);
			const retryManifest = compileDefaultCapabilityManifest({
				version: 1,
				role: "summary",
				state: "enabled",
				wireProtocol: "openai_chat_completions_v1",
				modelId: "db-test-retry-model",
				modelRevision: "1",
				endpointUrl: "https://summary.stub.invalid/v1/chat/completions",
				credentialRef: { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" },
			});
			const retryManifestFingerprint = retryManifest.configurationFingerprint;
			const retryProviderFingerprint = retryManifest.summaryProvider.providerFingerprint;
			const nextAdmission = migratedStore.admitRawEventFlushJob({
				source: "opencode",
				streamId: "completed-frontier-gap",
				manifestFingerprint: retryManifestFingerprint,
				providerFingerprint: retryProviderFingerprint,
			});
			expect(nextAdmission).toMatchObject({
				status: "admitted",
				startEventSeq: 2,
				endEventSeq: 2,
			});
			expect(
				migratedStore.admitRawEventFlushJob({
					source: "opencode",
					streamId: "completed-frontier-gap",
					manifestFingerprint: retryManifestFingerprint,
					providerFingerprint: retryProviderFingerprint,
				}),
			).toMatchObject({ status: "existing", jobId: nextAdmission.jobId });
			expect(
				migratedStore.confirmDoctorRetry({
					jobId: 1,
					producerReceiptId: "migrated-legacy-doctor-receipt",
					expectedRole: "summary",
					expectedProviderFingerprint: null,
					expectedManifestFingerprint: null,
					expectedAttemptCount: 0,
					expectedClaimGeneration: 0,
					targetProviderFingerprint: retryProviderFingerprint,
					targetManifestFingerprint: retryManifestFingerprint,
				}),
			).toMatchObject({ disposition: "accepted", grantState: "pending" });
			expect(
				migrated
					.prepare(
						`SELECT target_manifest_fingerprint, target_provider_fingerprint
						 FROM processing_resume_signals WHERE job_id = 1`,
					)
					.get(),
			).toEqual({
				target_manifest_fingerprint: retryManifestFingerprint,
				target_provider_fingerprint: retryProviderFingerprint,
			});
			const migratedClaim = migratedStore.claimRawEventFlushJob({
				jobId: 1,
				manifestFingerprint: retryManifestFingerprint,
				providerFingerprint: retryProviderFingerprint,
				manifest: retryManifest,
				boundary: compileProviderDestinationBoundary(retryManifest, {
					repositoryIdentity: migratedStore.rawEventFlushJobRepositoryIdentity(1),
					tlsPeerVerified: true,
				}),
			});
			if (!migratedClaim) throw new Error("expected migrated legacy claim");
			expect(
				migrated
					.prepare(
						`SELECT admission_manifest_fingerprint, admission_provider_fingerprint,
							attempt_manifest_fingerprint, attempt_provider_fingerprint, attempt_fingerprint
						 FROM raw_event_flush_batches WHERE id = 1`,
					)
					.get(),
			).toEqual({
				admission_manifest_fingerprint: null,
				admission_provider_fingerprint: null,
				attempt_manifest_fingerprint: retryManifestFingerprint,
				attempt_provider_fingerprint: retryProviderFingerprint,
				attempt_fingerprint: migratedClaim.attemptFingerprint,
			});
			expect(
				migrated
					.prepare(
						`SELECT error_message, error_type, observer_provider, observer_model,
								observer_runtime, observer_auth_source, observer_auth_type,
								observer_error_code, observer_error_message
							 FROM raw_event_flush_batches WHERE id = 1`,
					)
					.get(),
			).toEqual({
				error_message: "legacy-message",
				error_type: "legacy-type",
				observer_provider: "legacy-provider",
				observer_model: "legacy-model",
				observer_runtime: "legacy-runtime",
				observer_auth_source: "legacy-auth-source",
				observer_auth_type: "legacy-auth-type",
				observer_error_code: "legacy-code",
				observer_error_message: "legacy-observer-message",
			});
			expect(
				migrated
					.prepare(
						"SELECT stream_id, last_flushed_event_seq FROM raw_event_sessions ORDER BY stream_id",
					)
					.all(),
			).toEqual([
				{ stream_id: "complete", last_flushed_event_seq: 2 },
				{ stream_id: "completed-already-advanced", last_flushed_event_seq: 0 },
				{ stream_id: "completed-frontier-gap", last_flushed_event_seq: 1 },
				{ stream_id: "invalid-json", last_flushed_event_seq: -1 },
				{ stream_id: "missing", last_flushed_event_seq: 2 },
				{ stream_id: "overlap", last_flushed_event_seq: 3 },
				{ stream_id: "pending", last_flushed_event_seq: -1 },
			]);
			expect(
				migrated
					.prepare(
						`SELECT updated_at FROM raw_event_sessions
						 WHERE source = 'opencode' AND stream_id = 'completed-frontier-gap'`,
					)
					.get(),
			).toMatchObject({ updated_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T.*Z$/) });
			expect({
				retrievalAttempts: tableExists(migrated, "retrieval_attempts"),
				mutationReceipts: tableExists(migrated, "mutation_receipts"),
				compatibility: migrated
					.prepare("SELECT applied_schema_version FROM schema_compat_state WHERE id = 1")
					.get(),
			}).toEqual({
				retrievalAttempts: true,
				mutationReceipts: true,
				compatibility: { applied_schema_version: SCHEMA_VERSION },
			});

			const migratedDdl = normalizedSlice1Ddl(migrated);
			const migratedColumnShape = slice1ColumnShape(migrated);
			migrated.close();
			migrated = undefined;
			reopened = connect(join(tmpDir, "migrated.sqlite"));
			const reopenBackup = vi.fn(verifyTestBackup);
			runDatabaseMigrations(reopened, { dbPath: reopened.name, backupAndVerify: reopenBackup });
			expect(reopenBackup).not.toHaveBeenCalled();
			expect(normalizedSlice1Ddl(reopened)).toEqual(migratedDdl);

			fresh = connectMigrated(join(tmpDir, "fresh.sqlite"));
			expect(normalizedSlice1Ddl(fresh)).toEqual(migratedDdl);
			expect(slice1ColumnShape(fresh)).toEqual(migratedColumnShape);
		} finally {
			fresh?.close();
			reopened?.close();
			migrated?.close();
		}
	});

	it("migrates an already-flushed completed range after its source was pruned", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "completed-pruned.sqlite"));
		try {
			const now = "2026-08-31T00:00:00.000Z";
			legacy.exec(`
				INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				) VALUES ('opencode', 'completed-pruned', 'completed-pruned', 0, 0, '${now}');
				INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, created_at, updated_at
				) VALUES (6, 'opencode', 'completed-pruned', 'completed-pruned', 0, 0,
					'raw_events_v1', 'completed', '${now}', '${now}');
			`);

			runDatabaseMigrations(legacy, {
				dbPath: legacy.name,
				backupAndVerify: verifyTestBackup,
			});
			expect(getSchemaVersion(legacy)).toBe(SCHEMA_VERSION);
			expect(
				legacy
					.prepare(
						`SELECT status, completion_disposition, legacy_recovery_state
						 FROM raw_event_flush_batches WHERE id = 6`,
					)
					.get(),
			).toEqual({
				status: "completed",
				completion_disposition: "none",
				legacy_recovery_state: "not_legacy",
			});
			expect(
				legacy
					.prepare(
						`SELECT last_flushed_event_seq FROM raw_event_sessions
						 WHERE source = 'opencode' AND stream_id = 'completed-pruned'`,
					)
					.get(),
			).toEqual({ last_flushed_event_seq: 0 });
		} finally {
			legacy.close();
		}
	});

	it("preserves a complete legacy gave-up range wider than the v21 admission limit", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "wide-gave-up.sqlite"));
		try {
			const now = "2026-08-31T00:00:00.000Z";
			legacy
				.prepare(
					`INSERT INTO raw_event_sessions(
						source, stream_id, opencode_session_id, last_received_event_seq,
						last_flushed_event_seq, updated_at
					 ) VALUES ('opencode', 'wide-gave-up', 'wide-gave-up', 100, 100, ?)`,
				)
				.run(now);
			const insertEvent = legacy.prepare(
				`INSERT INTO raw_events(
					source, stream_id, opencode_session_id, event_id, event_seq,
					event_type, payload_json, created_at
				 ) VALUES ('opencode', 'wide-gave-up', 'wide-gave-up', ?, ?, 'message', '{}', ?)`,
			);
			legacy.transaction(() => {
				for (let eventSeq = 0; eventSeq <= 100; eventSeq++) {
					insertEvent.run(`wide-gave-up-${eventSeq}`, eventSeq, now);
				}
			})();
			legacy
				.prepare(
					`INSERT INTO raw_event_flush_batches(
						id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
						extractor_version, status, created_at, updated_at
					 ) VALUES (6, 'opencode', 'wide-gave-up', 'wide-gave-up', 0, 100,
						'raw_events_v1', 'gave_up', ?, ?)`,
				)
				.run(now, now);

			runDatabaseMigrations(legacy, {
				dbPath: legacy.name,
				backupAndVerify: verifyTestBackup,
			});

			expect(
				legacy
					.prepare(
						`SELECT status, completion_disposition, legacy_recovery_state,
							frontier_already_advanced
						 FROM raw_event_flush_batches WHERE id = 6`,
					)
					.get(),
			).toEqual({
				status: "retry_exhausted",
				completion_disposition: "none",
				legacy_recovery_state: "complete_range",
				frontier_already_advanced: 1,
			});
		} finally {
			legacy.close();
		}
	});

	it("rejects a fractional sequence in a legacy gave-up range", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "fractional-gave-up.sqlite"));
		try {
			const now = "2026-08-31T00:00:00.000Z";
			legacy.exec(`
				INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				) VALUES ('opencode', 'fractional-gave-up', 'fractional-gave-up', 2, 2, '${now}');
				INSERT INTO raw_events(
					source, stream_id, opencode_session_id, event_id, event_seq,
					event_type, payload_json, created_at
				) VALUES
					('opencode', 'fractional-gave-up', 'fractional-gave-up',
						'fractional-0', 0, 'message', '{}', '${now}'),
					('opencode', 'fractional-gave-up', 'fractional-gave-up',
						'fractional-half', 0.5, 'message', '{}', '${now}'),
					('opencode', 'fractional-gave-up', 'fractional-gave-up',
						'fractional-2', 2, 'message', '{}', '${now}');
				INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, created_at, updated_at
				) VALUES (6, 'opencode', 'fractional-gave-up', 'fractional-gave-up', 0, 2,
					'raw_events_v1', 'gave_up', '${now}', '${now}');
			`);

			runDatabaseMigrations(legacy, {
				dbPath: legacy.name,
				backupAndVerify: verifyTestBackup,
			});

			expect(
				legacy
					.prepare(
						`SELECT status, completion_disposition, legacy_recovery_state,
							frontier_already_advanced
						 FROM raw_event_flush_batches WHERE id = 6`,
					)
					.get(),
			).toEqual({
				status: "completed",
				completion_disposition: "legacy_unrecoverable",
				legacy_recovery_state: "missing_or_ambiguous_range",
				frontier_already_advanced: 0,
			});
		} finally {
			legacy.close();
		}
	});

	it("rolls v20 migration back when the next completed legacy range is incomplete", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "completed-gap.sqlite"));
		try {
			legacy.exec(`
				INSERT INTO raw_event_sessions(
					source, stream_id, opencode_session_id, last_received_event_seq,
					last_flushed_event_seq, updated_at
				) VALUES ('opencode', 'completed-gap', 'completed-gap', 2, -1,
					'2026-08-31T00:00:00.000Z');
				INSERT INTO raw_events(
					source, stream_id, opencode_session_id, event_id, event_seq, event_type,
					payload_json, created_at
				) VALUES
					('opencode', 'completed-gap', 'completed-gap', 'completed-gap-0', 0,
						'message', '{}', '2026-08-31T00:00:00.000Z'),
					('opencode', 'completed-gap', 'completed-gap', 'completed-gap-2', 2,
						'message', '{}', '2026-08-31T00:00:00.000Z');
				INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, created_at, updated_at
				) VALUES (6, 'opencode', 'completed-gap', 'completed-gap', 0, 2,
					'raw_events_v1', 'completed', '2026-08-31T00:00:00.000Z',
					'2026-08-31T00:00:00.000Z');
			`);

			expect(() =>
				runDatabaseMigrations(legacy, {
					dbPath: legacy.name,
					backupAndVerify: verifyTestBackup,
				}),
			).toThrow(/completed legacy range is incomplete or ambiguous/i);
			expect(getSchemaVersion(legacy)).toBe(20);
			expect(columnExists(legacy, "raw_event_flush_batches", "legacy_recovery_state")).toBe(false);
			expect(
				legacy
					.prepare(
						`SELECT status, start_event_seq, end_event_seq
						 FROM raw_event_flush_batches WHERE id = 6`,
					)
					.get(),
			).toEqual({ status: "completed", start_event_seq: 0, end_event_seq: 2 });
			expect(
				legacy
					.prepare(
						`SELECT last_flushed_event_seq FROM raw_event_sessions
						 WHERE source = 'opencode' AND stream_id = 'completed-gap'`,
					)
					.get(),
			).toEqual({ last_flushed_event_seq: -1 });
		} finally {
			legacy.close();
		}
	});

	it("rolls v20 migration back when a completed legacy range has no session frontier", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "completed-without-session.sqlite"));
		try {
			const now = "2026-08-31T00:00:00.000Z";
			legacy
				.prepare(
					`INSERT INTO raw_events(
						source, stream_id, opencode_session_id, event_id, event_seq, event_type,
						payload_json, created_at
					 ) VALUES ('opencode', 'completed-without-session', 'completed-without-session',
						'completed-without-session-0', 0, 'message', '{}', ?)`,
				)
				.run(now);
			legacy
				.prepare(
					`INSERT INTO raw_event_flush_batches(
						id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
						extractor_version, status, created_at, updated_at
					 ) VALUES (6, 'opencode', 'completed-without-session',
						'completed-without-session', 0, 0, 'raw_events_v1', 'completed', ?, ?)`,
				)
				.run(now, now);

			expect(() =>
				runDatabaseMigrations(legacy, {
					dbPath: legacy.name,
					backupAndVerify: verifyTestBackup,
				}),
			).toThrow(/completed legacy range is incomplete or ambiguous/i);
			expect(getSchemaVersion(legacy)).toBe(20);
			expect(columnExists(legacy, "raw_event_flush_batches", "legacy_recovery_state")).toBe(false);
			expect(
				legacy.prepare("SELECT status FROM raw_event_flush_batches WHERE id = 6").get(),
			).toEqual({ status: "completed" });
		} finally {
			legacy.close();
		}
	});

	it("rejects unsupported direct pre-v20 upgrades and mislabeled v21 schemas", () => {
		const path = join(tmpDir, "unsupported.sqlite");
		let legacy = seedV20Slice1Database(path);
		try {
			legacy.pragma("user_version = 18");
			legacy.pragma("journal_mode = DELETE");
			legacy.close();
			legacy = connect(path);
			expect(legacy.pragma("journal_mode", { simple: true })).toBe("delete");
			const backup = vi.fn(verifyTestBackup);
			expect(() =>
				runDatabaseMigrations(legacy, {
					dbPath: legacy.name,
					backupAndVerify: backup,
				}),
			).toThrow(/direct writable upgrade.*schema 20/i);
			expect(backup).not.toHaveBeenCalled();
			expect(getSchemaVersion(legacy)).toBe(18);
			expect(legacy.pragma("journal_mode", { simple: true })).toBe("delete");

			legacy.pragma("user_version = 21");
			expect(() => assertSchemaReady(legacy)).toThrow(/v21 schema is incomplete/i);
		} finally {
			legacy.close();
		}
	});

	it("migrates raw events and jobs in bounded pages", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "paged.sqlite"));
		try {
			const now = "2026-08-31T00:00:00.000Z";
			const addEvent = legacy.prepare(
				`INSERT INTO raw_events(
					source, stream_id, opencode_session_id, event_id, event_seq, event_type,
					payload_json, created_at
				 ) VALUES ('opencode', 'paged', 'paged', ?, ?, 'message', '{}', ?)`,
			);
			const addBatch = legacy.prepare(
				`INSERT INTO raw_event_flush_batches(
					id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
					extractor_version, status, created_at, updated_at
				 ) VALUES (?, 'opencode', 'paged', 'paged', ?, ?, 'raw_events_v1', 'completed', ?, ?)`,
			);
			const minimumId = "-9223372036854775808";
			legacy.transaction(() => {
				legacy
					.prepare(
						`INSERT INTO raw_event_sessions(
							source, stream_id, opencode_session_id, last_received_event_seq,
							last_flushed_event_seq, updated_at
						 ) VALUES ('opencode', 'paged', 'paged', 500, 500, ?)`,
					)
					.run(now);
				for (let index = 0; index < 501; index++) {
					addEvent.run(`paged-${index}`, index, now);
					addBatch.run(1_000 + index, index, index, now, now);
				}
				legacy
					.prepare(
						`INSERT INTO raw_event_sessions(
							source, stream_id, opencode_session_id, last_received_event_seq,
							last_flushed_event_seq, updated_at
						 ) VALUES ('opencode', 'minimum-id', 'minimum-id', 0, 0, ?)`,
					)
					.run(now);
				legacy
					.prepare(
						`INSERT INTO raw_events(
							id, source, stream_id, opencode_session_id, event_id, event_seq,
							event_type, payload_json, created_at
						 ) VALUES (CAST(? AS INTEGER), 'opencode', 'minimum-id', 'minimum-id',
							'minimum-id-event', 0, 'message', '{}', ?)`,
					)
					.run(minimumId, now);
				legacy
					.prepare(
						`INSERT INTO raw_event_flush_batches(
							id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
							extractor_version, status, created_at, updated_at
						 ) VALUES (CAST(? AS INTEGER), 'opencode', 'minimum-id', 'minimum-id', 0, 0,
							'raw_events_v1', 'completed', ?, ?)`,
					)
					.run(minimumId, now, now);
			})();
			const eventCount = (
				legacy.prepare("SELECT COUNT(*) AS count FROM raw_events").get() as { count: number }
			).count;
			const batchCount = (
				legacy.prepare("SELECT COUNT(*) AS count FROM raw_event_flush_batches").get() as {
					count: number;
				}
			).count;

			runDatabaseMigrations(legacy, {
				dbPath: legacy.name,
				backupAndVerify: verifyTestBackup,
			});

			expect(legacy.prepare("SELECT COUNT(*) AS count FROM raw_events").get()).toEqual({
				count: eventCount,
			});
			expect(legacy.prepare("SELECT COUNT(*) AS count FROM raw_event_flush_batches").get()).toEqual(
				{ count: batchCount },
			);
			expect(
				legacy.prepare("SELECT status FROM raw_event_flush_batches WHERE id = 1500").get(),
			).toEqual({ status: "completed" });
			expect(
				legacy
					.prepare(
						"SELECT CAST(id AS TEXT) AS id FROM raw_events WHERE event_id = 'minimum-id-event'",
					)
					.get(),
			).toEqual({ id: minimumId });
			expect(
				legacy
					.prepare(
						"SELECT CAST(id AS TEXT) AS id, status FROM raw_event_flush_batches WHERE id = CAST(? AS INTEGER)",
					)
					.get(minimumId),
			).toEqual({ id: minimumId, status: "completed" });
		} finally {
			legacy.close();
		}
	});

	it("keeps v20 unchanged when recoverable legacy jobs exceed durable capacity", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "over-capacity.sqlite"));
		try {
			const now = "2026-08-31T00:00:00.000Z";
			for (let index = 0; index < 24; index++) {
				const stream = `overflow-${index}`;
				legacy
					.prepare(
						`INSERT INTO raw_event_sessions(
							source, stream_id, opencode_session_id, last_flushed_event_seq, updated_at
						 ) VALUES ('opencode', ?, ?, -1, ?)`,
					)
					.run(stream, stream, now);
				legacy
					.prepare(
						`INSERT INTO raw_events(
							source, stream_id, opencode_session_id, event_id, event_seq,
							event_type, payload_json, created_at
						 ) VALUES ('opencode', ?, ?, ?, 0, 'message', '{}', ?)`,
					)
					.run(stream, stream, `${stream}-0`, now);
				legacy
					.prepare(
						`INSERT INTO raw_event_flush_batches(
							id, source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
							extractor_version, status, created_at, updated_at
						 ) VALUES (?, 'opencode', ?, ?, 0, 0, 'raw_events_v1', 'pending', ?, ?)`,
					)
					.run(100 + index, stream, stream, now, now);
			}
			expect(() =>
				runDatabaseMigrations(legacy, {
					dbPath: legacy.name,
					backupAndVerify: verifyTestBackup,
				}),
			).toThrow(/exceed the durable processing capacity/i);
			expect(getSchemaVersion(legacy)).toBe(20);
			expect(columnExists(legacy, "raw_event_flush_batches", "legacy_recovery_state")).toBe(false);
		} finally {
			legacy.close();
		}
	});

	it("identifies an invalid legacy raw-event row without exposing its labels", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "missing-event-id.sqlite"));
		try {
			legacy.exec("ALTER TABLE raw_events ADD COLUMN migration_id TEXT");
			const unsafe = legacy
				.prepare("SELECT source, stream_id FROM raw_events WHERE id = 1")
				.get() as {
				source: string;
				stream_id: string;
			};
			legacy
				.prepare(
					"UPDATE raw_events SET event_id = NULL, payload_json = ?, migration_id = ? WHERE id = 1",
				)
				.run('{"secret":"raw-payload-sentinel"}', "secret-label-sentinel");
			let thrown: unknown;
			try {
				runDatabaseMigrations(legacy, {
					dbPath: legacy.name,
					backupAndVerify: verifyTestBackup,
				});
			} catch (error) {
				thrown = error;
			}
			expect(thrown).toBeInstanceOf(Error);
			const message = thrown instanceof Error ? thrown.message : "";
			expect(message).toBe("v21 migration cannot preserve raw_events.id=1 without an event ID.");
			expect(message).not.toContain(unsafe.source);
			expect(message).not.toContain(unsafe.stream_id);
			expect(message).not.toContain("raw-payload-sentinel");
			expect(message).not.toContain("secret-label-sentinel");
			expect(getSchemaVersion(legacy)).toBe(20);
			expect(tableExists(legacy, "raw_events_v20")).toBe(false);
		} finally {
			legacy.close();
		}
	});

	it("rolls back v21 when required additive identity columns are unavailable", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "missing-additive-column.sqlite"));
		try {
			legacy.exec("ALTER TABLE memory_items DROP COLUMN import_key");
			expect(() =>
				runDatabaseMigrations(legacy, {
					dbPath: legacy.name,
					backupAndVerify: verifyTestBackup,
				}),
			).toThrow(/requires additive memory identity columns/i);
			expect(getSchemaVersion(legacy)).toBe(20);
			expect(tableExists(legacy, "raw_events_v20")).toBe(false);
			expect(tableExists(legacy, "raw_event_identity_conflicts")).toBe(false);
		} finally {
			legacy.close();
		}
	});

	it("rolls the entire v21 migration back for an unsupported legacy job state", () => {
		const legacy = seedV20Slice1Database(join(tmpDir, "unsupported-state.sqlite"));
		try {
			legacy.prepare("UPDATE raw_event_flush_batches SET status = 'mystery' WHERE id = 1").run();
			expect(() =>
				runDatabaseMigrations(legacy, {
					dbPath: legacy.name,
					backupAndVerify: verifyTestBackup,
				}),
			).toThrow(/unsupported legacy raw-event batch status/i);
			expect(getSchemaVersion(legacy)).toBe(20);
			expect(tableExists(legacy, "raw_events_v20")).toBe(false);
			expect(
				legacy
					.prepare(
						"SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'index' AND name = 'idx_raw_events_source_stream_event_id'",
					)
					.get(),
			).toEqual({ count: 1 });
		} finally {
			legacy.close();
		}
	});
});

describe("backupOnFirstAccess", () => {
	let tmpDir: string;
	let dbPath: string;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-backup-"));
		dbPath = join(tmpDir, "mem.sqlite");
		writeFileSync(dbPath, "test-db-content");
	});

	afterEach(() => {
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("creates marker and skips repeated backups", () => {
		backupOnFirstAccess(dbPath);
		const markerPath = join(tmpDir, ".codemem-ts-accessed");
		expect(existsSync(markerPath)).toBe(true);

		const firstBackups = readdirSync(tmpDir).filter(
			(name) => name.startsWith("mem.sqlite.pre-ts-") && name.endsWith(".bak"),
		);
		expect(firstBackups).toHaveLength(1);

		backupOnFirstAccess(dbPath);
		const secondBackups = readdirSync(tmpDir).filter(
			(name) => name.startsWith("mem.sqlite.pre-ts-") && name.endsWith(".bak"),
		);
		expect(secondBackups).toHaveLength(1);
	});

	it("writes marker when a viable pre-ts backup already exists", () => {
		const existingBackup = `${dbPath}.pre-ts-20260324T1710.bak`;
		writeFileSync(existingBackup, "test-db-content");

		backupOnFirstAccess(dbPath);

		const markerPath = join(tmpDir, ".codemem-ts-accessed");
		expect(existsSync(markerPath)).toBe(true);
		const backups = readdirSync(tmpDir).filter(
			(name) => name.startsWith("mem.sqlite.pre-ts-") && name.endsWith(".bak"),
		);
		expect(backups).toHaveLength(1);
	});

	it("skips backup when lock contention is active", () => {
		const lockPath = join(tmpDir, ".codemem-ts-backup.lock");
		writeFileSync(lockPath, "live");

		backupOnFirstAccess(dbPath);

		const markerPath = join(tmpDir, ".codemem-ts-accessed");
		expect(existsSync(markerPath)).toBe(false);
		const backups = readdirSync(tmpDir).filter(
			(name) => name.startsWith("mem.sqlite.pre-ts-") && name.endsWith(".bak"),
		);
		expect(backups).toHaveLength(0);
	});

	it("treats stale lock files as recoverable", () => {
		const lockPath = join(tmpDir, ".codemem-ts-backup.lock");
		writeFileSync(lockPath, "stale");
		const old = new Date(Date.now() - 20 * 60 * 1000);
		utimesSync(lockPath, old, old);

		backupOnFirstAccess(dbPath);

		const markerPath = join(tmpDir, ".codemem-ts-accessed");
		expect(existsSync(markerPath)).toBe(true);
		const backups = readdirSync(tmpDir).filter(
			(name) => name.startsWith("mem.sqlite.pre-ts-") && name.endsWith(".bak"),
		);
		expect(backups).toHaveLength(1);
	});
});

describe("loadSqliteVec", () => {
	let tmpDir: string;
	let db: Database;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
		db = new BetterSqlite3(join(tmpDir, "test.sqlite"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("loads the sqlite-vec extension", () => {
		loadSqliteVec(db);
		const row = db.prepare("SELECT vec_version() AS v").get() as { v: string };
		expect(row.v).toMatch(/^v?\d+\.\d+/);
	});

	it("skips loading when embeddings are disabled", () => {
		const orig = process.env.CODEMEM_EMBEDDING_DISABLED;
		try {
			process.env.CODEMEM_EMBEDDING_DISABLED = "1";
			// Should not throw, and vec_version should not be available
			loadSqliteVec(db);
			expect(() => db.prepare("SELECT vec_version()").get()).toThrow();
		} finally {
			if (orig === undefined) {
				delete process.env.CODEMEM_EMBEDDING_DISABLED;
			} else {
				process.env.CODEMEM_EMBEDDING_DISABLED = orig;
			}
		}
	});
});

describe("getSchemaVersion", () => {
	let tmpDir: string;
	let db: Database;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
		db = connectMigrated(join(tmpDir, "test.sqlite"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("returns the bootstrapped version for a fresh database", () => {
		expect(getSchemaVersion(db)).toBe(SCHEMA_VERSION);
	});

	it("returns the version after it is set", () => {
		db.pragma(`user_version = ${SCHEMA_VERSION}`);
		expect(getSchemaVersion(db)).toBe(SCHEMA_VERSION);
	});
});

describe("assertSchemaReady", () => {
	let tmpDir: string;
	let db: Database;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
		db = connectMigrated(join(tmpDir, "test.sqlite"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("bootstraps uninitialized writable schemas before asserting readiness", () => {
		const uninitialized = connect(join(tmpDir, "uninitialized.sqlite"));
		try {
			expect(getSchemaVersion(uninitialized)).toBe(0);
			expect(() => assertSchemaReady(uninitialized)).toThrow(/not initialized/);
			runDatabaseMigrations(uninitialized, {
				dbPath: uninitialized.name,
				backupAndVerify: verifyTestBackup,
			});
			expect(() => assertSchemaReady(uninitialized)).not.toThrow();
			expect(getSchemaVersion(uninitialized)).toBe(SCHEMA_VERSION);
			expect(tableExists(uninitialized, "memory_items")).toBe(true);
		} finally {
			uninitialized.close();
		}
	});

	it("does not bootstrap unrelated non-empty SQLite databases", () => {
		const unrelated = new BetterSqlite3(join(tmpDir, "unrelated.sqlite"));
		try {
			unrelated.exec("CREATE TABLE unrelated_data (id INTEGER PRIMARY KEY)");
			expect(getSchemaVersion(unrelated)).toBe(0);

			expect(() => assertSchemaReady(unrelated)).toThrow(/not initialized/);
			expect(tableExists(unrelated, "memory_items")).toBe(false);
			expect(tableExists(unrelated, "unrelated_data")).toBe(true);
		} finally {
			unrelated.close();
		}
	});

	it("does not try to bootstrap readonly uninitialized schemas", () => {
		const dbPath = join(tmpDir, "readonly-uninitialized.sqlite");
		const seed = new BetterSqlite3(dbPath);
		seed.close();
		const readonly = new BetterSqlite3(dbPath, { readonly: true });
		try {
			expect(getSchemaVersion(readonly)).toBe(0);
			expect(() => assertSchemaReady(readonly)).toThrow(/not initialized/);
			expect(() => assertSchemaReady(readonly)).not.toThrow(/readonly/i);
		} finally {
			readonly.close();
		}
	});

	it("passes for the current schema version with required tables", () => {
		expect(() => assertSchemaReady(db)).not.toThrow();
	});

	it("throws for a stale schema version", () => {
		db.pragma("user_version = 3");
		expect(() => assertSchemaReady(db)).toThrow(/older than minimum compatible/);
	});

	it("warns but continues for a newer schema version", () => {
		db.pragma(`user_version = ${SCHEMA_VERSION + 1}`);
		expect(() => assertSchemaReady(db)).not.toThrow();
	});

	it("routes newer read-only schema warnings through the caller-provided sink", () => {
		const warn = vi.fn();
		db.pragma(`user_version = ${SCHEMA_VERSION + 1}`);

		expect(() => assertSchemaReadyReadOnly(db, warn)).not.toThrow();
		expect(warn).toHaveBeenCalledOnce();
		expect(warn.mock.calls[0]?.[0]).toContain("newer than this TS runtime");
	});

	it("throws when required tables are missing", () => {
		const missingTables = new BetterSqlite3(join(tmpDir, "missing-tables.sqlite"));
		try {
			missingTables.pragma(`user_version = ${SCHEMA_VERSION}`);
			expect(() => assertSchemaReady(missingTables)).toThrow(/Required tables missing/);
		} finally {
			missingTables.close();
		}
	});
});

describe("tableExists", () => {
	let tmpDir: string;
	let db: Database;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
		db = connectMigrated(join(tmpDir, "test.sqlite"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("returns false for a non-existent table", () => {
		expect(tableExists(db, "nonexistent")).toBe(false);
	});

	it("returns true for an existing table", () => {
		db.exec("CREATE TABLE test_table (id INTEGER PRIMARY KEY)");
		expect(tableExists(db, "test_table")).toBe(true);
	});
});

describe("ensureAdditiveSchemaCompatibility", () => {
	let tmpDir: string;
	let db: Database;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
		db = new BetterSqlite3(join(tmpDir, "test.sqlite"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("adds missing raw_event_flush_batches compatibility columns", () => {
		db.exec(`
			CREATE TABLE raw_event_flush_batches (
				id INTEGER PRIMARY KEY,
				source TEXT NOT NULL,
				stream_id TEXT NOT NULL,
				opencode_session_id TEXT NOT NULL,
				start_event_seq INTEGER NOT NULL,
				end_event_seq INTEGER NOT NULL,
				extractor_version TEXT NOT NULL,
				status TEXT NOT NULL,
				created_at TEXT NOT NULL,
				updated_at TEXT NOT NULL
			)
		`);

		expect(columnExists(db, "raw_event_flush_batches", "error_message")).toBe(false);
		expect(columnExists(db, "raw_event_flush_batches", "attempt_count")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(columnExists(db, "raw_event_flush_batches", "error_message")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "error_type")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "observer_provider")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "observer_model")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "observer_runtime")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "observer_auth_source")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "observer_auth_type")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "observer_error_code")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "observer_error_message")).toBe(true);
		expect(columnExists(db, "raw_event_flush_batches", "attempt_count")).toBe(true);
	});

	it("adds memory_items dedup_key column and index", () => {
		db.exec(`
			CREATE TABLE memory_items (
				id INTEGER PRIMARY KEY,
				session_id INTEGER NOT NULL,
				kind TEXT NOT NULL,
				title TEXT NOT NULL,
				body_text TEXT NOT NULL,
				visibility TEXT,
				workspace_id TEXT,
				active INTEGER DEFAULT 1,
				created_at TEXT NOT NULL,
				updated_at TEXT NOT NULL
			)
		`);

		expect(columnExists(db, "memory_items", "dedup_key")).toBe(false);
		expect(hasIndex(db, "idx_memory_items_dedup_key_active_created")).toBe(false);
		expect(hasIndex(db, "idx_memory_items_same_session_dedup_unique")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(columnExists(db, "memory_items", "dedup_key")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_dedup_key_active_created")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_same_session_dedup_unique")).toBe(true);
	});

	it("treats duplicate-column races as benign when column now exists", () => {
		let racedColumnVisible = false;
		let alterAttempts = 0;

		const fakeDb = {
			// Legacy user_version so the additive shim runs (does not short-circuit).
			pragma() {
				return 0;
			},
			prepare(query: string) {
				return {
					get(...args: unknown[]) {
						if (query.includes("sqlite_master")) {
							return { ok: 1 };
						}
						if (query.includes("pragma_table_info")) {
							const requestedColumn = String(args[1] ?? "");
							if (requestedColumn === "error_message") {
								return racedColumnVisible ? { ok: 1 } : undefined;
							}
							return { ok: 1 };
						}
						return undefined;
					},
				};
			},
			exec(sqlText: string) {
				if (sqlText.includes("ADD COLUMN error_message")) {
					alterAttempts += 1;
					racedColumnVisible = true;
					throw new Error("duplicate column name: error_message");
				}
			},
		} as unknown as Database;

		expect(() => ensureAdditiveSchemaCompatibility(fakeDb)).not.toThrow();
		expect(alterAttempts).toBe(1);
	});

	it("is a no-op when raw_event_flush_batches does not exist", () => {
		expect(() => ensureAdditiveSchemaCompatibility(db)).not.toThrow();
	});

	it("adds sync_peers discovery-provenance columns and creates coordinator_group_preferences", () => {
		db.exec(`
			CREATE TABLE sync_peers (
				peer_device_id TEXT PRIMARY KEY NOT NULL,
				name TEXT,
				pinned_fingerprint TEXT,
				public_key TEXT,
				addresses_json TEXT,
				claimed_local_actor INTEGER NOT NULL DEFAULT 0,
				actor_id TEXT,
				projects_include_json TEXT,
				projects_exclude_json TEXT,
				created_at TEXT NOT NULL,
				last_seen_at TEXT,
				last_sync_at TEXT,
				last_error TEXT
			)
		`);

		expect(columnExists(db, "sync_peers", "discovered_via_coordinator_id")).toBe(false);
		expect(columnExists(db, "sync_peers", "discovered_via_group_id")).toBe(false);
		expect(columnExists(db, "sync_peers", "trust_provenance")).toBe(false);
		expect(tableExists(db, "coordinator_group_preferences")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(columnExists(db, "sync_peers", "discovered_via_coordinator_id")).toBe(true);
		expect(columnExists(db, "sync_peers", "discovered_via_group_id")).toBe(true);
		expect(columnExists(db, "sync_peers", "trust_provenance")).toBe(true);
		expect(tableExists(db, "coordinator_group_preferences")).toBe(true);
		expect(columnExists(db, "coordinator_group_preferences", "default_space_scope_id")).toBe(true);
		expect(
			columnExists(db, "coordinator_group_preferences", "auto_grant_default_space_on_join"),
		).toBe(true);

		db.exec(
			"INSERT INTO coordinator_group_preferences " +
				"(coordinator_id, group_id, auto_seed_scope, updated_at) " +
				"VALUES ('https://coord.example', 'team-alpha', 1, '2026-04-23T00:00:00Z')",
		);
		const row = db
			.prepare("SELECT coordinator_id, group_id FROM coordinator_group_preferences")
			.get() as { coordinator_id: string; group_id: string };
		expect(row.coordinator_id).toBe("https://coord.example");
		expect(row.group_id).toBe("team-alpha");
	});

	it("adds sync_attempts capability diagnostics columns on legacy schemas", () => {
		db.exec(`
			CREATE TABLE sync_attempts (
				id INTEGER PRIMARY KEY AUTOINCREMENT,
				peer_device_id TEXT NOT NULL,
				started_at TEXT NOT NULL,
				finished_at TEXT,
				ok INTEGER NOT NULL DEFAULT 0,
				ops_in INTEGER NOT NULL DEFAULT 0,
				ops_out INTEGER NOT NULL DEFAULT 0,
				error TEXT
			)
		`);

		expect(columnExists(db, "sync_attempts", "local_sync_capability")).toBe(false);
		expect(columnExists(db, "sync_attempts", "peer_sync_capability")).toBe(false);
		expect(columnExists(db, "sync_attempts", "negotiated_sync_capability")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(columnExists(db, "sync_attempts", "local_sync_capability")).toBe(true);
		expect(columnExists(db, "sync_attempts", "peer_sync_capability")).toBe(true);
		expect(columnExists(db, "sync_attempts", "negotiated_sync_capability")).toBe(true);
	});

	it("creates replication scope tables and indexes on legacy schemas", () => {
		expect(tableExists(db, "replication_scopes")).toBe(false);
		expect(tableExists(db, "project_scope_mappings")).toBe(false);
		expect(tableExists(db, "scope_memberships")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);
		ensureAdditiveSchemaCompatibility(db);

		expect(tableExists(db, "replication_scopes")).toBe(true);
		expect(tableExists(db, "project_scope_mappings")).toBe(true);
		expect(tableExists(db, "scope_memberships")).toBe(true);
		expect(hasIndex(db, "idx_replication_scopes_status")).toBe(true);
		expect(hasIndex(db, "idx_replication_scopes_authority_group")).toBe(true);
		expect(columnInfo(db, "replication_scopes", "scope_id")?.is_not_null).toBe(1);
		expect(hasIndex(db, "idx_project_scope_mappings_workspace_priority")).toBe(true);
		expect(hasIndex(db, "idx_project_scope_mappings_pattern_priority")).toBe(true);
		expect(hasIndex(db, "idx_project_scope_mappings_scope")).toBe(true);
		expect(hasIndex(db, "idx_scope_memberships_device_status")).toBe(true);
		expect(hasIndex(db, "idx_scope_memberships_scope_status")).toBe(true);
		expect(hasIndex(db, "idx_scope_memberships_authority_group")).toBe(true);

		db.prepare(
			`INSERT INTO replication_scopes
				(scope_id, label, kind, authority_type, membership_epoch, status, created_at, updated_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
		).run(
			"local-default",
			"Local only",
			"system",
			"local",
			0,
			"active",
			"2026-04-30T00:00:00Z",
			"2026-04-30T00:00:00Z",
		);
		const row = db.prepare("SELECT label, authority_type FROM replication_scopes").get() as
			| { label: string; authority_type: string }
			| undefined;
		expect(row).toEqual({ label: "Local only", authority_type: "local" });
	});

	it("creates missing scope indexes when scope tables already exist", () => {
		db.exec(`
			CREATE TABLE replication_scopes (
				scope_id TEXT PRIMARY KEY NOT NULL,
				label TEXT NOT NULL,
				kind TEXT NOT NULL DEFAULT 'user',
				authority_type TEXT NOT NULL DEFAULT 'local',
				coordinator_id TEXT,
				group_id TEXT,
				manifest_issuer_device_id TEXT,
				membership_epoch INTEGER NOT NULL DEFAULT 0,
				manifest_hash TEXT,
				status TEXT NOT NULL DEFAULT 'active',
				created_at TEXT NOT NULL,
				updated_at TEXT NOT NULL
			);
			CREATE TABLE project_scope_mappings (
				id INTEGER PRIMARY KEY,
				workspace_identity TEXT,
				project_pattern TEXT NOT NULL,
				scope_id TEXT NOT NULL,
				priority INTEGER NOT NULL DEFAULT 0,
				source TEXT NOT NULL DEFAULT 'user',
				created_at TEXT NOT NULL,
				updated_at TEXT NOT NULL
			);
			CREATE TABLE scope_memberships (
				scope_id TEXT NOT NULL,
				device_id TEXT NOT NULL,
				role TEXT NOT NULL DEFAULT 'member',
				status TEXT NOT NULL DEFAULT 'active',
				membership_epoch INTEGER NOT NULL DEFAULT 0,
				coordinator_id TEXT,
				group_id TEXT,
				manifest_issuer_device_id TEXT,
				manifest_hash TEXT,
				signed_manifest_json TEXT,
				updated_at TEXT NOT NULL,
				PRIMARY KEY (scope_id, device_id)
			);
		`);

		expect(hasIndex(db, "idx_replication_scopes_status")).toBe(false);
		expect(hasIndex(db, "idx_project_scope_mappings_scope")).toBe(false);
		expect(hasIndex(db, "idx_scope_memberships_scope_status")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(hasIndex(db, "idx_replication_scopes_status")).toBe(true);
		expect(hasIndex(db, "idx_project_scope_mappings_scope")).toBe(true);
		expect(hasIndex(db, "idx_scope_memberships_scope_status")).toBe(true);
		expect(columnInfo(db, "replication_scopes", "scope_id")?.is_not_null).toBe(1);
	});

	it("creates and seeds per-scope sync state tables from legacy state", () => {
		db.exec(`
			CREATE TABLE sync_reset_state (
				id INTEGER PRIMARY KEY,
				generation INTEGER NOT NULL,
				snapshot_id TEXT NOT NULL,
				baseline_cursor TEXT,
				retained_floor_cursor TEXT,
				updated_at TEXT NOT NULL
			);
			INSERT INTO sync_reset_state
				(id, generation, snapshot_id, baseline_cursor, retained_floor_cursor, updated_at)
			VALUES
				(1, 7, 'snapshot-legacy', 'cursor-baseline', 'cursor-floor', '2026-04-30T00:00:00Z');

			CREATE TABLE sync_retention_state (
				id INTEGER PRIMARY KEY,
				last_run_at TEXT,
				last_duration_ms INTEGER,
				last_deleted_ops INTEGER NOT NULL DEFAULT 0,
				last_estimated_bytes_before INTEGER,
				last_estimated_bytes_after INTEGER,
				retained_floor_cursor TEXT,
				last_error TEXT,
				last_error_at TEXT
			);
			INSERT INTO sync_retention_state
				(
					id,
					last_run_at,
					last_duration_ms,
					last_deleted_ops,
					last_estimated_bytes_before,
					last_estimated_bytes_after,
					retained_floor_cursor,
					last_error,
					last_error_at
				)
			VALUES
				(
					1,
					'2026-04-30T01:00:00Z',
					42,
					3,
					1000,
					700,
					'cursor-floor',
					NULL,
					NULL
				);

			CREATE TABLE replication_cursors (
				peer_device_id TEXT PRIMARY KEY,
				last_applied_cursor TEXT,
				last_acked_cursor TEXT,
				updated_at TEXT NOT NULL
			);
			INSERT INTO replication_cursors
				(peer_device_id, last_applied_cursor, last_acked_cursor, updated_at)
			VALUES
				('peer-a', 'op-10', 'op-9', '2026-04-30T02:00:00Z');
		`);

		expect(tableExists(db, "sync_reset_state_v2")).toBe(false);
		expect(tableExists(db, "sync_retention_state_v2")).toBe(false);
		expect(tableExists(db, "replication_cursors_v2")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);
		ensureAdditiveSchemaCompatibility(db);

		expect(tableExists(db, "sync_reset_state_v2")).toBe(true);
		expect(tableExists(db, "sync_retention_state_v2")).toBe(true);
		expect(tableExists(db, "replication_cursors_v2")).toBe(true);
		expect(hasIndex(db, "idx_replication_cursors_v2_scope")).toBe(true);
		expect(columnInfo(db, "sync_reset_state_v2", "scope_id")?.is_not_null).toBe(1);
		expect(columnInfo(db, "replication_cursors_v2", "scope_id")?.is_not_null).toBe(1);

		const reset = db
			.prepare("SELECT * FROM sync_reset_state_v2 WHERE scope_id = ?")
			.get("local-default") as {
			generation: number;
			snapshot_id: string;
			baseline_cursor: string;
			retained_floor_cursor: string;
			updated_at: string;
		};
		expect(reset).toMatchObject({
			generation: 7,
			snapshot_id: "snapshot-legacy",
			baseline_cursor: "cursor-baseline",
			retained_floor_cursor: "cursor-floor",
			updated_at: "2026-04-30T00:00:00Z",
		});

		const retention = db
			.prepare("SELECT * FROM sync_retention_state_v2 WHERE scope_id = ?")
			.get("local-default") as {
			last_run_at: string;
			last_duration_ms: number;
			last_deleted_ops: number;
			last_estimated_bytes_before: number;
			last_estimated_bytes_after: number;
			retained_floor_cursor: string;
		};
		expect(retention).toMatchObject({
			last_run_at: "2026-04-30T01:00:00Z",
			last_duration_ms: 42,
			last_deleted_ops: 3,
			last_estimated_bytes_before: 1000,
			last_estimated_bytes_after: 700,
			retained_floor_cursor: "cursor-floor",
		});

		const cursor = db
			.prepare("SELECT * FROM replication_cursors_v2 WHERE peer_device_id = ? AND scope_id = ?")
			.get("peer-a", "local-default") as {
			last_applied_cursor: string;
			last_acked_cursor: string;
			updated_at: string;
		};
		expect(cursor).toMatchObject({
			last_applied_cursor: "op-10",
			last_acked_cursor: "op-9",
			updated_at: "2026-04-30T02:00:00Z",
		});

		db.prepare(
			"INSERT INTO sync_reset_state_v2 (scope_id, generation, snapshot_id, updated_at) VALUES (?, ?, ?, ?)",
		).run("work-scope", 1, "snapshot-work", "2026-04-30T03:00:00Z");
		expect(
			db.prepare("SELECT scope_id FROM sync_reset_state_v2 ORDER BY scope_id").pluck().all(),
		).toEqual(["local-default", "work-scope"]);
	});

	it("adds nullable scope columns and indexes on legacy memory/op tables", () => {
		db.exec(`
			CREATE TABLE memory_items (
				id INTEGER PRIMARY KEY,
				session_id INTEGER NOT NULL,
				kind TEXT NOT NULL,
				title TEXT NOT NULL,
				body_text TEXT NOT NULL,
				visibility TEXT,
				workspace_id TEXT,
				active INTEGER DEFAULT 1,
				created_at TEXT NOT NULL,
				updated_at TEXT NOT NULL
			);
			CREATE TABLE replication_ops (
				op_id TEXT PRIMARY KEY,
				entity_type TEXT NOT NULL,
				entity_id TEXT NOT NULL,
				op_type TEXT NOT NULL,
				payload_json TEXT,
				clock_rev INTEGER NOT NULL,
				clock_updated_at TEXT NOT NULL,
				clock_device_id TEXT NOT NULL,
				device_id TEXT NOT NULL,
				created_at TEXT NOT NULL
			);
		`);

		expect(columnExists(db, "memory_items", "scope_id")).toBe(false);
		expect(columnExists(db, "replication_ops", "scope_id")).toBe(false);
		expect(hasIndex(db, "idx_memory_items_scope_visibility_created")).toBe(false);
		expect(hasIndex(db, "idx_memory_items_scope_backfill_pending")).toBe(false);
		expect(hasIndex(db, "idx_replication_ops_scope_created")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);
		ensureAdditiveSchemaCompatibility(db);

		expect(columnExists(db, "memory_items", "scope_id")).toBe(true);
		expect(columnExists(db, "replication_ops", "scope_id")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_scope_visibility_created")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_scope_backfill_pending")).toBe(true);
		expect(hasIndex(db, "idx_replication_ops_scope_created")).toBe(true);
	});

	it("creates memory_file_refs and memory_concept_refs on v6 databases missing them", () => {
		// Simulate a v6 database that has memory_items but lacks the junction tables.
		db.exec(`
			CREATE TABLE memory_items (
				id INTEGER PRIMARY KEY,
				session_id INTEGER NOT NULL,
				kind TEXT NOT NULL,
				title TEXT NOT NULL,
				body_text TEXT NOT NULL,
				visibility TEXT,
				workspace_id TEXT,
				active INTEGER DEFAULT 1,
				created_at TEXT NOT NULL,
				updated_at TEXT NOT NULL
			)
		`);
		db.pragma("user_version = 6");

		expect(tableExists(db, "memory_file_refs")).toBe(false);
		expect(tableExists(db, "memory_concept_refs")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(tableExists(db, "memory_file_refs")).toBe(true);
		expect(tableExists(db, "memory_concept_refs")).toBe(true);
		expect(hasIndex(db, "idx_memory_file_refs_path")).toBe(true);
		expect(hasIndex(db, "idx_memory_concept_refs_concept")).toBe(true);
	});

	it("adds memory_items.project column and backfills from sessions.project", () => {
		db.exec(`
			CREATE TABLE sessions (
				id INTEGER PRIMARY KEY,
				started_at TEXT NOT NULL,
				cwd TEXT,
				project TEXT,
				user TEXT,
				tool_version TEXT
			)
		`);
		db.exec(`
			CREATE TABLE memory_items (
				id INTEGER PRIMARY KEY,
				session_id INTEGER NOT NULL,
				kind TEXT NOT NULL,
				title TEXT NOT NULL,
				body_text TEXT NOT NULL,
				visibility TEXT,
				workspace_id TEXT,
				active INTEGER DEFAULT 1,
				created_at TEXT NOT NULL,
				updated_at TEXT NOT NULL,
				FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
			)
		`);
		const now = new Date().toISOString();
		db.prepare(
			`INSERT INTO sessions(started_at, cwd, project) VALUES (?, '/work/codemem', 'codemem')`,
		).run(now);
		db.prepare(
			`INSERT INTO sessions(started_at, cwd, project) VALUES (?, '/work/other', NULL)`,
		).run(now);
		db.prepare(
			`INSERT INTO memory_items(session_id, kind, title, body_text, created_at, updated_at)
			 VALUES (1, 'discovery', 'a', 'b', ?, ?)`,
		).run(now, now);
		db.prepare(
			`INSERT INTO memory_items(session_id, kind, title, body_text, created_at, updated_at)
			 VALUES (2, 'discovery', 'a', 'b', ?, ?)`,
		).run(now, now);

		expect(columnExists(db, "memory_items", "project")).toBe(false);
		expect(hasIndex(db, "idx_memory_items_project")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(columnExists(db, "memory_items", "project")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_project")).toBe(true);
		const rows = db
			.prepare("SELECT session_id, project FROM memory_items ORDER BY session_id")
			.all() as Array<{ session_id: number; project: string | null }>;
		expect(rows).toEqual([
			{ session_id: 1, project: "codemem" },
			{ session_id: 2, project: null },
		]);

		// Idempotent — running the migration again is a no-op for already-set
		// project rows and does not error.
		expect(() => ensureAdditiveSchemaCompatibility(db)).not.toThrow();
		const rows2 = db
			.prepare("SELECT session_id, project FROM memory_items ORDER BY session_id")
			.all() as Array<{ session_id: number; project: string | null }>;
		expect(rows2).toEqual(rows);
	});
});

describe("ensureAdditiveSchemaCompatibility schema-compat gate", () => {
	let tmpDir: string;
	let db: Database;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
		// Start with a bootstrapped schema before the compatibility marker exists.
		db = connectBootstrapped(join(tmpDir, "test.sqlite"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	function appliedSchemaVersion(database: Database): number | undefined {
		const row = database
			.prepare("SELECT applied_schema_version AS v FROM schema_compat_state WHERE id = 1")
			.get() as { v: number } | undefined;
		return row?.v;
	}

	it("applies and marks the schema-compat state on a legacy database", () => {
		// Simulate a legacy DB so the version short-circuit does not fire.
		db.pragma("user_version = 6");
		expect(tableExists(db, "schema_compat_state")).toBe(false);

		ensureAdditiveSchemaCompatibility(db);

		expect(tableExists(db, "schema_compat_state")).toBe(true);
		expect(tableExists(db, "share_operations")).toBe(true);
		expect(tableExists(db, "share_operation_projects")).toBe(true);
		expect(tableExists(db, "share_operation_steps")).toBe(true);
		expect(tableExists(db, "recipient_policy_review_resolutions")).toBe(true);
		for (const table of [
			"coordinator_enrollment_reconciliation_issues",
			"policy_teams",
			"policy_team_memberships",
			"identity_devices",
			"project_recipients",
			"recipient_managed_project_projections",
			"recipient_policy_authority_states",
			"recipient_policy_reconciliation_steps",
			"recipient_policy_deny_overlays",
		]) {
			expect(tableExists(db, table)).toBe(true);
		}
		expect(tableExists(db, "identities")).toBe(false);
		expect(
			db.prepare("SELECT COUNT(*) FROM recipient_managed_project_projections").pluck().get(),
		).toBe(0);
		expect(appliedSchemaVersion(db)).toBe(SCHEMA_VERSION);
	});

	it("upgrades a database marked at the previous schema version and remains idempotent", () => {
		db.close();
		const dbPath = join(tmpDir, "schema-previous.sqlite");
		const previous = new BetterSqlite3(dbPath);
		previous.exec(`
			PRAGMA user_version = ${SCHEMA_VERSION - 1};
			CREATE TABLE schema_compat_state (
				id INTEGER PRIMARY KEY,
				applied_schema_version INTEGER NOT NULL,
				applied_at TEXT NOT NULL
			);
			INSERT INTO schema_compat_state VALUES (1, ${SCHEMA_VERSION - 1}, '2026-07-19T00:00:00Z');
		`);
		expect(tableExists(previous, "share_operations")).toBe(false);

		ensureAdditiveSchemaCompatibility(previous);

		for (const table of ["share_operations", "share_operation_projects", "share_operation_steps"]) {
			expect(tableExists(previous, table)).toBe(true);
		}
		expect(tableExists(previous, "recipient_policy_review_resolutions")).toBe(true);
		for (const table of [
			"coordinator_enrollment_reconciliation_issues",
			"policy_teams",
			"policy_team_memberships",
			"identity_devices",
			"project_recipients",
			"recipient_managed_project_projections",
			"recipient_policy_authority_states",
			"recipient_policy_reconciliation_steps",
			"recipient_policy_deny_overlays",
		]) {
			expect(tableExists(previous, table)).toBe(true);
		}
		expect(tableExists(previous, "identities")).toBe(false);
		expect(getSchemaVersion(previous)).toBe(SCHEMA_VERSION);
		expect(columnExists(previous, "share_operations", "pending_person_operation_id")).toBe(true);
		expect(columnExists(previous, "share_operations", "recipient_device_id")).toBe(true);
		expect(columnExists(previous, "share_operations", "bootstrap_grant_id")).toBe(true);
		expect(columnExists(previous, "share_operation_projects", "existing_memory_count")).toBe(true);
		expect(columnExists(previous, "share_operation_steps", "effect_id")).toBe(true);
		expect(hasIndex(previous, "idx_share_operations_state_updated")).toBe(true);
		expect(hasIndex(previous, "idx_share_operations_invite_digest")).toBe(true);
		expect(hasIndex(previous, "idx_share_operations_pending_person_operation")).toBe(true);
		expect(hasIndex(previous, "idx_coordinator_enrollment_issues_boundary_status")).toBe(true);
		expect(hasIndex(previous, "idx_coordinator_enrollment_issues_status_recent")).toBe(true);
		expect(hasIndex(previous, "idx_recipient_managed_projects_identity_status")).toBe(true);
		expect(hasIndex(previous, "idx_recipient_managed_projects_scope_authority")).toBe(true);
		expect(hasIndex(previous, "idx_recipient_policy_reconciliation_steps_pending_refresh")).toBe(
			true,
		);
		expect(appliedSchemaVersion(previous)).toBe(SCHEMA_VERSION);
		previous
			.prepare(`INSERT INTO coordinator_enrollment_reconciliation_issues(
			coordinator_id, group_id, kind, reference_id, code, status,
			first_seen_at, last_seen_at, occurrence_count, updated_at
		) VALUES ('coordinator', 'group', 'device', 'device', 'safe_code', 'open',
			'2026-07-29T00:00:00Z', '2026-07-29T00:00:00Z', 1, '2026-07-29T00:00:00Z')`)
			.run();
		previous
			.prepare(
				`INSERT INTO share_operations(
				operation_id, state, inviter_actor_id, inviter_device_ids_json, person_id,
				person_kind, pending_person_operation_id, teammate_name, history_policy,
				reviewed_project_set_digest, coordinator_group_id, coordinator_invite_id,
				invite_token_digest, invite_expires_at, created_at, updated_at
			 ) VALUES ('share_test', 'waiting_for_acceptance', 'actor', '[]', 'person',
				'pending', 'share_test', 'Brian', 'existing_and_future', 'digest', 'group',
				'invite', 'token-digest', '2099-01-01T00:00:00Z', '2026-07-20T00:00:00Z',
				'2026-07-20T00:00:00Z')`,
			)
			.run();
		previous.close();

		const reopened = new BetterSqlite3(dbPath);
		ensureAdditiveSchemaCompatibility(reopened);
		expect(reopened.prepare("SELECT COUNT(*) FROM share_operations").pluck().get()).toBe(1);
		expect(
			reopened
				.prepare("SELECT COUNT(*) FROM coordinator_enrollment_reconciliation_issues")
				.pluck()
				.get(),
		).toBe(1);
		expect(appliedSchemaVersion(reopened)).toBe(SCHEMA_VERSION);
		reopened.close();
		db = connectMigrated(join(tmpDir, "replacement.sqlite"));
	});

	it("upgrades a schema-v14 marker before using managed Project projections", () => {
		db.close();
		const previous = new BetterSqlite3(join(tmpDir, "schema-v14-projection.sqlite"));
		previous.exec(`
			PRAGMA user_version = 14;
			CREATE TABLE schema_compat_state (
				id INTEGER PRIMARY KEY,
				applied_schema_version INTEGER NOT NULL,
				applied_at TEXT NOT NULL
			);
			INSERT INTO schema_compat_state VALUES (1, 14, '2026-07-29T00:00:00Z');
		`);

		ensureAdditiveSchemaCompatibility(previous);

		expect(tableExists(previous, "recipient_managed_project_projections")).toBe(true);
		expect(hasIndex(previous, "idx_recipient_managed_projects_identity_status")).toBe(true);
		expect(hasIndex(previous, "idx_recipient_managed_projects_scope_authority")).toBe(true);
		expect(getSchemaVersion(previous)).toBe(SCHEMA_VERSION);
		expect(appliedSchemaVersion(previous)).toBe(SCHEMA_VERSION);
		previous.close();
		db = connectMigrated(join(tmpDir, "replacement.sqlite"));
	});

	it("repairs partially-created share operation tables before marking compatibility", () => {
		db.close();
		const partial = new BetterSqlite3(join(tmpDir, "partial.sqlite"));
		partial.exec(`
			PRAGMA user_version = 8;
			CREATE TABLE schema_compat_state (
				id INTEGER PRIMARY KEY,
				applied_schema_version INTEGER NOT NULL,
				applied_at TEXT NOT NULL
			);
			INSERT INTO schema_compat_state VALUES (1, 8, '2026-07-19T00:00:00Z');
			CREATE TABLE share_operations (operation_id TEXT PRIMARY KEY NOT NULL);
			CREATE TABLE share_operation_projects (
				operation_id TEXT NOT NULL,
				canonical_project_identity TEXT NOT NULL,
				PRIMARY KEY (operation_id, canonical_project_identity)
			);
			CREATE TABLE share_operation_steps (
				operation_id TEXT NOT NULL,
				step_key TEXT NOT NULL,
				PRIMARY KEY (operation_id, step_key)
			);
		`);

		ensureAdditiveSchemaCompatibility(partial);

		expect(columnExists(partial, "share_operations", "invite_token_digest")).toBe(true);
		expect(columnExists(partial, "share_operations", "recipient_actor_id")).toBe(true);
		expect(columnExists(partial, "share_operations", "recipient_fingerprint")).toBe(true);
		expect(columnExists(partial, "share_operation_projects", "existing_memory_count")).toBe(true);
		expect(columnExists(partial, "share_operation_steps", "safe_error_code")).toBe(true);
		expect(hasIndex(partial, "idx_share_operations_invite_digest")).toBe(true);
		expect(hasIndex(partial, "idx_share_operation_steps_effect_id_nonempty")).toBe(true);
		partial
			.prepare(
				`INSERT INTO share_operation_steps(operation_id, step_key, effect_id)
				 VALUES (?, ?, ?)`,
			)
			.run("share-one", "step-one", "effect-one");
		partial
			.prepare(
				`INSERT INTO share_operation_steps(operation_id, step_key, effect_id)
				 VALUES (?, ?, ?)`,
			)
			.run("share-two", "step-two", "effect-one");
		expect(
			partial
				.prepare("SELECT COUNT(*) FROM share_operation_steps WHERE effect_id = ?")
				.pluck()
				.get("effect-one"),
		).toBe(2);
		expect(appliedSchemaVersion(partial)).toBe(SCHEMA_VERSION);
		partial.close();
		db = connectMigrated(join(tmpDir, "replacement.sqlite"));
	});

	it("repairs a previously applied unique effect-id constraint", () => {
		const stale = new BetterSqlite3(join(tmpDir, "stale-share-effect.sqlite"));
		try {
			stale.exec(`
				CREATE TABLE schema_compat_state (
					id INTEGER PRIMARY KEY CHECK (id = 1),
					applied_schema_version INTEGER NOT NULL,
					applied_at TEXT NOT NULL
				);
				INSERT INTO schema_compat_state VALUES (1, ${SCHEMA_VERSION}, '2026-07-20T00:00:00Z');
				CREATE TABLE share_operation_steps (
					operation_id TEXT NOT NULL,
					step_key TEXT NOT NULL,
					effect_id TEXT NOT NULL UNIQUE,
					status TEXT NOT NULL,
					attempt_count INTEGER NOT NULL DEFAULT 0,
					started_at TEXT,
					completed_at TEXT,
					last_attempt_at TEXT,
					safe_error_code TEXT,
					updated_at TEXT NOT NULL,
					PRIMARY KEY (operation_id, step_key)
				);
				CREATE UNIQUE INDEX idx_share_operation_steps_effect_id_nonempty
					ON share_operation_steps(effect_id) WHERE effect_id <> '';
				INSERT INTO share_operation_steps(
					operation_id, step_key, effect_id, status, updated_at
				) VALUES ('share-one', 'step-one', 'shared-effect', 'pending', '2026-07-20T00:00:00Z');
			`);

			ensureAdditiveSchemaCompatibility(stale);
			stale
				.prepare(`INSERT INTO share_operation_steps(
					operation_id, step_key, effect_id, status, updated_at
				) VALUES (?, ?, ?, 'pending', ?)`)
				.run("share-two", "step-two", "shared-effect", "2026-07-20T00:01:00Z");

			expect(
				stale
					.prepare("SELECT COUNT(*) FROM share_operation_steps WHERE effect_id = ?")
					.pluck()
					.get("shared-effect"),
			).toBe(2);
		} finally {
			stale.close();
		}
	});

	it("skips gated DDL once marked and re-applies on version mismatch", () => {
		db.pragma("user_version = 6");
		ensureAdditiveSchemaCompatibility(db);
		expect(hasIndex(db, "idx_memory_items_project")).toBe(true);

		// Drop an additive index. Because the marker now says applied-for-this
		// version, the next call must skip the gated DDL and NOT recreate it.
		db.exec("DROP INDEX idx_memory_items_project");
		expect(hasIndex(db, "idx_memory_items_project")).toBe(false);
		ensureAdditiveSchemaCompatibility(db);
		expect(hasIndex(db, "idx_memory_items_project")).toBe(false);

		// Roll the marker back to simulate an older/again-needed schema; the gated
		// DDL must re-run and recreate the index.
		db.exec("UPDATE schema_compat_state SET applied_schema_version = 0");
		ensureAdditiveSchemaCompatibility(db);
		expect(hasIndex(db, "idx_memory_items_project")).toBe(true);
		expect(appliedSchemaVersion(db)).toBe(SCHEMA_VERSION);
	});

	it("repairs an early v21 resume receipt table after the compatibility gate is marked", () => {
		ensureAdditiveSchemaCompatibility(db);
		db.exec("ALTER TABLE processing_resume_producer_receipts DROP COLUMN target_job_ids_json");
		db.prepare(
			`INSERT INTO processing_resume_producer_receipts(
				receipt_id, producer_kind, configuration_fingerprint, provider_fingerprint,
				producer_sequence, fanout_count, created_at
			 ) VALUES (?, 'validated_configuration_activation', ?, ?, 42, 0, ?)`,
		).run(
			"early-v21-activation",
			`sha256:${"a".repeat(64)}`,
			`sha256:${"b".repeat(64)}`,
			"2026-08-31T00:00:00.000Z",
		);
		const dbPath = db.name;
		db.close();
		db = connect(dbPath);
		const backupAndVerify = vi.fn(verifyTestBackup);

		runDatabaseMigrations(db, { dbPath, backupAndVerify });

		expect(backupAndVerify).toHaveBeenCalledOnce();
		expect(columnExists(db, "processing_resume_producer_receipts", "target_job_ids_json")).toBe(
			true,
		);
		expect(
			db
				.prepare(
					"SELECT target_job_ids_json FROM processing_resume_producer_receipts WHERE receipt_id = ?",
				)
				.get("early-v21-activation"),
		).toEqual({ target_job_ids_json: "[]" });
		expect(
			new MemoryStore(db, { closeConnection: false }).importActivationReceipt({
				receiptId: "early-v21-activation",
				activationSequence: 42,
				manifestFingerprint: `sha256:${"a".repeat(64)}`,
				providerFingerprint: `sha256:${"b".repeat(64)}`,
			}),
		).toMatchObject({ disposition: "duplicate", fanoutCount: 0, results: [] });
	});

	it("runs the project backfill even when gated DDL is skipped", () => {
		db.pragma("user_version = 6");
		// First call marks the schema-compat state so the next open is gated.
		ensureAdditiveSchemaCompatibility(db);

		const now = new Date().toISOString();
		db.prepare(
			"INSERT INTO sessions(id, started_at, cwd, project) VALUES (1, ?, '/work/codemem', 'codemem')",
		).run(now);
		db.prepare(
			`INSERT INTO memory_items(session_id, kind, title, body_text, created_at, updated_at, project)
			 VALUES (1, 'discovery', 'a', 'b', ?, ?, NULL)`,
		).run(now, now);

		// This open is gated (DDL skipped) but the backfill must still run.
		ensureAdditiveSchemaCompatibility(db);

		const row = db.prepare("SELECT project FROM memory_items WHERE session_id = 1").get() as {
			project: string | null;
		};
		expect(row.project).toBe("codemem");
	});

	it("applies and marks on a fresh DB (gate is marker-only, not user_version)", () => {
		// A freshly bootstrapped DB is at user_version=SCHEMA_VERSION, but the gate
		// keys on the marker, not the version: the shim runs once (its statements
		// are all idempotent no-ops here) and records the marker.
		expect(getSchemaVersion(db)).toBe(SCHEMA_VERSION);
		expect(tableExists(db, "schema_compat_state")).toBe(false);
		ensureAdditiveSchemaCompatibility(db);
		expect(tableExists(db, "schema_compat_state")).toBe(true);
		expect(appliedSchemaVersion(db)).toBe(SCHEMA_VERSION);
	});

	it("re-adds a missing additive column even at user_version=SCHEMA_VERSION", () => {
		// Regression: additive columns (e.g. memory_items.project) were added over
		// time WITHOUT bumping SCHEMA_VERSION, so a DB can report
		// user_version=SCHEMA_VERSION yet still lack one. Gating on user_version
		// would skip the shim and leave the column missing, breaking inserts that
		// reference it. The marker-only gate must still repair it.
		expect(getSchemaVersion(db)).toBe(SCHEMA_VERSION);
		db.exec("DROP INDEX IF EXISTS idx_memory_items_project");
		db.exec("ALTER TABLE memory_items DROP COLUMN project");
		expect(columnExists(db, "memory_items", "project")).toBe(false);

		// No marker exists, so the shim runs despite user_version=SCHEMA_VERSION.
		ensureAdditiveSchemaCompatibility(db);

		expect(columnExists(db, "memory_items", "project")).toBe(true);
		expect(hasIndex(db, "idx_memory_items_project")).toBe(true);
		expect(appliedSchemaVersion(db)).toBe(SCHEMA_VERSION);
	});

	it("schemaCompatAlreadyApplied-style probe is fail-safe without the table", () => {
		// On a DB lacking schema_compat_state, a legacy open must still apply the
		// shim (the gate returns false on the missing table rather than skipping).
		const legacy = new BetterSqlite3(join(tmpDir, "legacy.sqlite"));
		try {
			legacy.exec(`
				CREATE TABLE memory_items (
					id INTEGER PRIMARY KEY,
					session_id INTEGER NOT NULL,
					kind TEXT NOT NULL,
					title TEXT NOT NULL,
					body_text TEXT NOT NULL,
					visibility TEXT,
					workspace_id TEXT,
					active INTEGER DEFAULT 1,
					created_at TEXT NOT NULL,
					updated_at TEXT NOT NULL
				)
			`);
			// user_version defaults to 0 (legacy) and schema_compat_state is absent.
			expect(tableExists(legacy, "schema_compat_state")).toBe(false);
			expect(() => ensureAdditiveSchemaCompatibility(legacy)).not.toThrow();
			// The shim ran (fail-safe gate), creating + marking the state table.
			expect(tableExists(legacy, "schema_compat_state")).toBe(true);
			expect(columnExists(legacy, "memory_items", "project")).toBe(true);
		} finally {
			legacy.close();
		}
	});
});

describe("ensurePlannerStats", () => {
	let tmpDir: string;
	let db: Database;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-test-"));
		db = new BetterSqlite3(join(tmpDir, "test.sqlite"));
	});

	afterEach(() => {
		db?.close();
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("is a no-op for fresh databases without search tables", () => {
		expect(() => ensurePlannerStats(db)).not.toThrow();
		expect(tableExists(db, "sqlite_stat1")).toBe(false);
	});

	it("bootstraps sqlite_stat1 once search tables exist", () => {
		db.exec(
			"CREATE TABLE memory_items (id INTEGER PRIMARY KEY, active INTEGER NOT NULL DEFAULT 1, created_at TEXT, title TEXT, body_text TEXT, tags_text TEXT)",
		);
		db.exec(
			"CREATE VIRTUAL TABLE memory_fts USING fts5(title, body_text, tags_text, content='memory_items', content_rowid='id')",
		);
		db.exec("CREATE INDEX idx_memory_items_active_created ON memory_items(active, created_at)");

		expect(tableExists(db, "sqlite_stat1")).toBe(false);

		ensurePlannerStats(db);

		expect(tableExists(db, "sqlite_stat1")).toBe(true);
		expect(db.prepare("SELECT 1 FROM sqlite_stat1 LIMIT 1").pluck().get()).toBe(1);
	});
});

describe("JSON helpers", () => {
	it("fromJson returns {} for null/empty", () => {
		expect(fromJson(null)).toEqual({});
		expect(fromJson(undefined)).toEqual({});
		expect(fromJson("")).toEqual({});
	});

	it("fromJson parses valid JSON", () => {
		expect(fromJson('{"key": "value"}')).toEqual({ key: "value" });
	});

	it("fromJson returns {} for invalid JSON", () => {
		expect(fromJson("not json")).toEqual({});
	});

	it("toJson serializes to JSON string", () => {
		expect(toJson({ key: "value" })).toBe('{"key":"value"}');
	});

	it("toJson returns {} for null/undefined", () => {
		expect(toJson(null)).toBe("{}");
		expect(toJson(undefined)).toBe("{}");
	});
});

describe("isEmbeddingDisabled", () => {
	const envKey = "CODEMEM_EMBEDDING_DISABLED";
	let orig: string | undefined;

	beforeEach(() => {
		orig = process.env[envKey];
		delete process.env[envKey];
	});

	afterEach(() => {
		if (orig === undefined) {
			delete process.env[envKey];
		} else {
			process.env[envKey] = orig;
		}
	});

	it("returns false when env var is unset", () => {
		expect(isEmbeddingDisabled()).toBe(false);
	});

	it('returns true for "1"', () => {
		process.env[envKey] = "1";
		expect(isEmbeddingDisabled()).toBe(true);
	});

	it('returns true for "true" (case-insensitive)', () => {
		process.env[envKey] = "TRUE";
		expect(isEmbeddingDisabled()).toBe(true);
	});

	it('returns true for "yes"', () => {
		process.env[envKey] = "yes";
		expect(isEmbeddingDisabled()).toBe(true);
	});

	it('returns false for "0"', () => {
		process.env[envKey] = "0";
		expect(isEmbeddingDisabled()).toBe(false);
	});
});

describe("migrateLegacyDbPath", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = mkdtempSync(join(tmpdir(), "codemem-migrate-"));
	});

	afterEach(() => {
		rmSync(tmpDir, { recursive: true, force: true });
	});

	it("moves a legacy file to the target path", () => {
		const legacyPath = join(tmpDir, "legacy.sqlite");
		const legacy = new BetterSqlite3(legacyPath);
		legacy.exec("CREATE TABLE legacy_guard (value TEXT NOT NULL)");
		legacy.close();
		const layout = resolveStorageLayout(join(tmpDir, "target"));

		runLegacyMigration({
			layout,
			operationId: "legacy-test",
			verifiedBackupPath: legacyPath,
			verifiedBackupSha256: sha256File(legacyPath),
		});

		expect(readlinkSync(layout.currentPointerPath)).toBe("versions/legacy-test.sqlite");
		expect(existsSync(join(layout.versionsDir, "legacy-test.sqlite"))).toBe(true);
	});

	it("skips migration when target already exists", () => {
		const legacyPath = join(tmpDir, "legacy.sqlite");
		const legacy = new BetterSqlite3(legacyPath);
		legacy.exec("CREATE TABLE legacy_guard (value TEXT NOT NULL)");
		legacy.close();
		const layout = resolveStorageLayout(join(tmpDir, "target"));
		const input = {
			layout,
			operationId: "legacy-first",
			verifiedBackupPath: legacyPath,
			verifiedBackupSha256: sha256File(legacyPath),
		};
		runLegacyMigration(input);

		expect(() => runLegacyMigration({ ...input, operationId: "legacy-second" })).toThrow(
			/existing current database pointer/,
		);
		expect(readlinkSync(layout.currentPointerPath)).toBe("versions/legacy-first.sqlite");
	});
});
