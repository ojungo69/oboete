import { createHash } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readlinkSync,
	rmSync,
	statSync,
	symlinkSync,
	unlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import BetterSqlite3 from "better-sqlite3";
import { afterEach, describe, expect, it } from "vitest";
import { connect } from "./db.js";
import * as core from "./index.js";
import { openTestMemoryStore } from "./test-utils.js";

type StorageLayout = {
	controlDir: string;
	dbDir: string;
	versionsDir: string;
	currentPointerPath: string;
	journalPath: string;
};

type StorageJournal = {
	version: 1;
	operationId: string;
	state: "prepared" | "switched" | "committed";
	oldPointer: string | null;
	newPointer: string;
	artifactSha256: string;
};

const createdDirs: string[] = [];

function tempDir(prefix: string): string {
	const dir = mkdtempSync(join(tmpdir(), prefix));
	createdDirs.push(dir);
	return dir;
}

function sha256(path: string): string {
	return createHash("sha256").update(readFileSync(path)).digest("hex");
}

afterEach(() => {
	delete process.env.CODEMEM_DB_OPEN_TRACE;
	for (const dir of createdDirs.splice(0)) {
		rmSync(dir, { recursive: true, force: true });
	}
});

describe("Phase 1 daemon foundation", () => {
	it("P1-T033-01-single-writer", () => {
		const dir = tempDir("codemem-writer-actor-");
		const tracePath = join(dir, "db-open.jsonl");
		const dbPath = join(dir, "memory.sqlite");
		process.env.CODEMEM_DB_OPEN_TRACE = tracePath;

		const db = connect(dbPath);
		try {
			expect(db.constructor.name).toBe("WriterActor");
		} finally {
			db.close();
		}

		const trace = readFileSync(tracePath, "utf8")
			.trim()
			.split("\n")
			.map((line) => JSON.parse(line) as Record<string, unknown>);
		expect(trace).toEqual([
			expect.objectContaining({
				version: 1,
				event: "sqlite_open",
				mode: "writer",
				owner: "writer_actor",
				pid: process.pid,
				dbPath: resolve(dbPath),
			}),
		]);
	});

	it("P1-T033-02-migration-gate", () => {
		const runDatabaseMigrations = Reflect.get(core, "runDatabaseMigrations") as
			| ((
					db: ReturnType<typeof connect>,
					options: {
						dbPath: string;
						backupAndVerify: (context: { db: ReturnType<typeof connect> }) => {
							verified: boolean;
							evidence: string;
						};
					},
			  ) => void)
			| undefined;
		expect(runDatabaseMigrations).toBeTypeOf("function");

		const dir = tempDir("codemem-migration-gate-");
		const dbPath = join(dir, "allowed.sqlite");
		const db = connect(dbPath);
		try {
			runDatabaseMigrations?.(db, {
				dbPath,
				backupAndVerify: ({ db: gatedDb }) => {
					expect(core.getSchemaVersion(gatedDb)).toBe(0);
					expect(core.tableExists(gatedDb, "memory_items")).toBe(false);
					return { verified: true, evidence: "fresh-database-test" };
				},
			});
			expect(core.getSchemaVersion(db)).toBe(core.SCHEMA_VERSION);
			expect(core.tableExists(db, "memory_items")).toBe(true);
		} finally {
			db.close();
		}

		const rejectedPath = join(dir, "rejected.sqlite");
		const rejectedDb = connect(rejectedPath);
		try {
			expect(() =>
				runDatabaseMigrations?.(rejectedDb, {
					dbPath: rejectedPath,
					backupAndVerify: () => ({ verified: false, evidence: "verification-failed" }),
				}),
			).toThrow(/verified backup/i);
			expect(core.getSchemaVersion(rejectedDb)).toBe(0);
			expect(core.tableExists(rejectedDb, "memory_items")).toBe(false);
		} finally {
			rejectedDb.close();
		}
	});

	it("P1-T033-03-no-raw-db-export", () => {
		const dir = tempDir("codemem-public-db-");
		const store = openTestMemoryStore(join(dir, "memory.sqlite"));
		try {
			expect(Reflect.get(core, "Database")).toBeUndefined();
			expect(Reflect.get(core, "MemoryStore")).toBeUndefined();
			expect(store.db).not.toBeInstanceOf(BetterSqlite3);
			expect(Reflect.get(store.db, "raw")).toBeUndefined();
			expect(Object.values(store.db).some((value) => value instanceof BetterSqlite3)).toBe(false);
			expect(store.db.exec("SELECT 1")).toBe(store.db);
			const statement = store.db.prepare("SELECT 1");
			expect(statement.database).toBe(store.db);
			expect(statement.database).not.toBeInstanceOf(BetterSqlite3);
			const transaction = store.db.transaction(() => 1);
			expect(Reflect.get(transaction, "database")).toBe(store.db);
			for (const variant of [
				transaction.default,
				transaction.deferred,
				transaction.immediate,
				transaction.exclusive,
			]) {
				expect(Reflect.get(variant, "database")).not.toBeInstanceOf(BetterSqlite3);
			}
			expect(transaction()).toBe(1);
		} finally {
			store.close();
		}
	});

	it("P1-T033-04-storage-journal-recovery", () => {
		const resolveStorageLayout = Reflect.get(core, "resolveStorageLayout") as
			| ((dataDir: string) => StorageLayout)
			| undefined;
		const runLegacyMigration = Reflect.get(core, "runLegacyMigration") as
			| ((input: {
					layout: StorageLayout;
					operationId: string;
					verifiedBackupPath: string;
					verifiedBackupSha256: string;
			  }) => void)
			| undefined;
		const writeStorageJournal = Reflect.get(core, "writeStorageJournal") as
			| ((layout: StorageLayout, journal: StorageJournal) => void)
			| undefined;
		const recoverStorageJournal = Reflect.get(core, "recoverStorageJournal") as
			| ((layout: StorageLayout) => { action: string })
			| undefined;

		expect(resolveStorageLayout).toBeTypeOf("function");
		expect(runLegacyMigration).toBeTypeOf("function");
		expect(writeStorageJournal).toBeTypeOf("function");
		expect(recoverStorageJournal).toBeTypeOf("function");

		const dir = tempDir("codemem-storage-recovery-");
		const layout = resolveStorageLayout?.(join(dir, "data"));
		expect(layout).toBeDefined();
		if (!layout) return;

		const legacyPath = join(dir, "legacy.sqlite");
		const legacyDb = new BetterSqlite3(legacyPath);
		legacyDb.exec("CREATE TABLE recovery_guard (value TEXT NOT NULL)");
		legacyDb.prepare("INSERT INTO recovery_guard(value) VALUES (?)").run("old");
		legacyDb.close();
		runLegacyMigration?.({
			layout,
			operationId: "legacy-setup",
			verifiedBackupPath: legacyPath,
			verifiedBackupSha256: sha256(legacyPath),
		});

		const oldPointer = "versions/legacy-setup.sqlite";
		expect(readlinkSync(layout.currentPointerPath)).toBe(oldPointer);
		expect(statSync(layout.dataDir).mode & 0o777).toBe(0o700);
		expect(statSync(layout.controlDir).mode & 0o777).toBe(0o700);
		expect(statSync(layout.versionsDir).mode & 0o777).toBe(0o700);
		expect(statSync(join(layout.dbDir, oldPointer)).mode & 0o777).toBe(0o600);

		mkdirSync(layout.versionsDir, { recursive: true });
		const newPointer = "versions/replacement.sqlite";
		const replacementPath = join(layout.dbDir, newPointer);
		const replacementDb = new BetterSqlite3(replacementPath);
		replacementDb.exec("CREATE TABLE recovery_guard (value TEXT NOT NULL)");
		replacementDb.prepare("INSERT INTO recovery_guard(value) VALUES (?)").run("new");
		replacementDb.close();
		const artifactSha256 = sha256(replacementPath);

		writeStorageJournal?.(layout, {
			version: 1,
			operationId: "prepared-crash",
			state: "prepared",
			oldPointer,
			newPointer,
			artifactSha256,
		});
		expect(recoverStorageJournal?.(layout)).toEqual({ action: "rolled_back", state: "prepared" });
		expect(readlinkSync(layout.currentPointerPath)).toBe(oldPointer);
		expect(existsSync(layout.journalPath)).toBe(false);

		unlinkSync(layout.currentPointerPath);
		symlinkSync(newPointer, layout.currentPointerPath);
		writeStorageJournal?.(layout, {
			version: 1,
			operationId: "switched-crash",
			state: "switched",
			oldPointer,
			newPointer,
			artifactSha256,
		});
		expect(recoverStorageJournal?.(layout)).toEqual({ action: "rolled_back", state: "switched" });
		expect(readlinkSync(layout.currentPointerPath)).toBe(oldPointer);
		expect(existsSync(layout.journalPath)).toBe(false);

		unlinkSync(layout.currentPointerPath);
		symlinkSync(newPointer, layout.currentPointerPath);
		writeStorageJournal?.(layout, {
			version: 1,
			operationId: "committed-crash",
			state: "committed",
			oldPointer,
			newPointer,
			artifactSha256,
		});
		expect(recoverStorageJournal?.(layout)).toEqual({ action: "completed", state: "committed" });
		expect(readlinkSync(layout.currentPointerPath)).toBe(newPointer);
		expect(existsSync(layout.journalPath)).toBe(false);
	});
});
