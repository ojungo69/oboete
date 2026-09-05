import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import {
	existsSync,
	linkSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	readlinkSync,
	rmSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { startDaemon } from "./daemon-lifecycle.js";
import { captureManagedTarget, writeInstallManifest } from "./install-manifest.js";
import { cutoverLegacyDatabase, listOpenFileOwners } from "./legacy-cutover.js";
import { runDatabaseMigrations } from "./migration-runner.js";
import {
	readCurrentDatabasePointer,
	resolveRuntimeDataDir,
	resolveStorageLayout,
} from "./storage.js";
import { ReadOnlyActor, WriterActor } from "./writer-actor.js";

const require = createRequire(import.meta.url);
const sqliteModule = require.resolve("better-sqlite3");
const dirs: string[] = [];
const children: ChildProcess[] = [];

function fixture(customDatabase = false, legacyParentRuntime = false) {
	const root = mkdtempSync(join(tmpdir(), "codemem-legacy-cutover-"));
	dirs.push(root);
	const legacyPath = customDatabase
		? join(root, "custom.sqlite")
		: join(root, "data", "mem.sqlite");
	const layout = resolveStorageLayout(
		customDatabase
			? legacyParentRuntime
				? root
				: resolveRuntimeDataDir({ dbPath: legacyPath })
			: join(root, "data"),
	);
	if (!layout.dataDir.startsWith(`${root}/`) && layout.dataDir !== root) dirs.push(layout.dataDir);
	const db = WriterActor.open(legacyPath);
	runDatabaseMigrations(db, {
		dbPath: legacyPath,
		backupAndVerify: () => ({ verified: true, evidence: "fresh-test-database" }),
	});
	db.exec("CREATE TABLE legacy_probe (value TEXT NOT NULL)");
	db.prepare("INSERT INTO legacy_probe(value) VALUES (?)").run("preserved");
	db.close();
	const managedPath = join(root, "thin-client.sh");
	writeFileSync(managedPath, "#!/bin/sh\nexec codemem codex-hook-ingest\n", { mode: 0o600 });
	writeInstallManifest(layout.installManifestPath, {
		version: 1,
		blocks: [],
		targets: [captureManagedTarget("thin-client", managedPath)],
	});
	return { root, layout, legacyPath, managedPath };
}

function spawnIdleReader(path: string): Promise<ChildProcess> {
	const script = `
		const Database = require(${JSON.stringify(sqliteModule)});
		const db = new Database(process.argv[1], { readonly: true, fileMustExist: true });
		process.send?.("ready");
		process.on("SIGTERM", () => { db.close(); process.exit(0); });
		setInterval(() => {}, 1000);
	`;
	const child = spawn(process.execPath, ["-e", script, path], {
		stdio: ["ignore", "ignore", "ignore", "ipc"],
	});
	children.push(child);
	return new Promise((resolve, reject) => {
		child.once("message", () => resolve(child));
		child.once("error", reject);
		child.once("exit", (code) => reject(new Error(`idle reader exited early (${code})`)));
	});
}

function stopChild(child: ChildProcess): Promise<void> {
	if (child.exitCode !== null) return Promise.resolve();
	return new Promise((resolveStop) => {
		child.once("exit", () => resolveStop());
		child.kill("SIGTERM");
	});
}

afterEach(() => {
	for (const child of children.splice(0)) {
		if (child.exitCode === null) child.kill("SIGKILL");
	}
	for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("Phase 1 legacy layout cutover", () => {
	it("P1-T051-01-legacy-owner-set", async () => {
		const { legacyPath } = fixture();
		const reader = await spawnIdleReader(legacyPath);
		const local = WriterActor.open(legacyPath);
		try {
			expect(listOpenFileOwners(legacyPath).map((owner) => owner.pid)).toEqual(
				[process.pid, reader.pid].sort((left, right) => left - right),
			);
		} finally {
			local.close();
		}
	});

	it("P1-T051-02-cutover-fail-closed", async () => {
		const { layout, legacyPath, managedPath } = fixture();
		const reader = await spawnIdleReader(legacyPath);

		await expect(cutoverLegacyDatabase({ layout, legacyPath })).rejects.toThrow(
			/open handle|owner/i,
		);
		expect(readCurrentDatabasePointer(layout)).toBeNull();
		expect(lstatSync(legacyPath).isFile()).toBe(true);

		await stopChild(reader);
		await expect(
			cutoverLegacyDatabase({
				layout,
				legacyPath,
				onStep: (event) => {
					if (event !== "final_owner_scan") return;
					writeFileSync(
						managedPath,
						readFileSync(managedPath, "utf8").replace("codex-hook-ingest", "legacy-direct-db"),
						{ mode: 0o600 },
					);
				},
			}),
		).rejects.toThrow(/manifest/i);
		expect(readCurrentDatabasePointer(layout)).toBeNull();
		expect(lstatSync(legacyPath).isFile()).toBe(true);
	});

	it("P1-T051-03-tombstone-before-unlock", async () => {
		const { layout, legacyPath } = fixture();
		const events: string[] = [];
		let pointerAtTombstone: string | null | undefined;

		await cutoverLegacyDatabase({
			layout,
			legacyPath,
			onStep: (event) => {
				events.push(event);
				if (event === "tombstone_installed") {
					pointerAtTombstone = readCurrentDatabasePointer(layout);
				}
			},
		});
		expect(pointerAtTombstone).toBeNull();
		expect(events.indexOf("tombstone_installed")).toBeGreaterThanOrEqual(0);
		expect(events.indexOf("final_owner_scan")).toBeGreaterThan(
			events.indexOf("tombstone_installed"),
		);
		expect(events.indexOf("legacy_handles_closed")).toBeGreaterThan(
			events.indexOf("final_owner_scan"),
		);
		expect(lstatSync(legacyPath).isSymbolicLink()).toBe(true);
		expect(lstatSync(readlinkSync(legacyPath)).isDirectory()).toBe(true);
		expect(listOpenFileOwners(legacyPath)).toEqual([]);

		const interrupted = fixture(true, true);
		const recoveryPath = join(
			interrupted.layout.controlDir,
			"legacy-00000000-0000-4000-8000-000000000001.legacy-recovery.sqlite",
		);
		const duplicateRecoveryPath = join(
			interrupted.layout.controlDir,
			"legacy-00000000-0000-4000-8000-000000000002.legacy-recovery.sqlite",
		);
		const tombstoneDir = join(interrupted.layout.controlDir, "legacy-db-tombstone");
		linkSync(interrupted.legacyPath, recoveryPath);
		linkSync(interrupted.legacyPath, duplicateRecoveryPath);
		mkdirSync(tombstoneDir, { recursive: true, mode: 0o700 });
		rmSync(interrupted.legacyPath);
		symlinkSync(tombstoneDir, interrupted.legacyPath, "dir");

		const interruptedDb = process.env.CODEMEM_DB;
		process.env.CODEMEM_DB = interrupted.legacyPath;
		let restarted: Awaited<ReturnType<typeof startDaemon>> | undefined;
		try {
			const interruptedDataDir = resolveRuntimeDataDir({ dbPath: interrupted.legacyPath });
			expect(interruptedDataDir).toBe(interrupted.layout.dataDir);
			restarted = await startDaemon({ dataDir: interruptedDataDir });
			await restarted.stop();
			const interruptedPointer = readCurrentDatabasePointer(interrupted.layout);
			expect(interruptedPointer).not.toBeNull();
			const recovered = ReadOnlyActor.open(
				join(interrupted.layout.dbDir, interruptedPointer as string),
			);
			try {
				expect(recovered.prepare("SELECT value FROM legacy_probe").pluck().get()).toBe("preserved");
			} finally {
				recovered.close();
			}
			expect(lstatSync(interrupted.legacyPath).isSymbolicLink()).toBe(true);
			expect(
				readdirSync(interrupted.layout.controlDir).filter((name) =>
					name.endsWith(".legacy-recovery.sqlite"),
				),
			).toEqual([]);
		} finally {
			await restarted?.stop().catch(() => {});
			if (interruptedDb === undefined) delete process.env.CODEMEM_DB;
			else process.env.CODEMEM_DB = interruptedDb;
		}

		const missing = fixture();
		const missingTombstone = join(missing.layout.controlDir, "legacy-db-tombstone");
		mkdirSync(missingTombstone, { recursive: true, mode: 0o700 });
		rmSync(missing.legacyPath);
		symlinkSync(missingTombstone, missing.legacyPath, "dir");
		await expect(startDaemon({ dataDir: missing.layout.dataDir })).rejects.toThrow(
			/recovery file is missing/i,
		);
		expect(readCurrentDatabasePointer(missing.layout)).toBeNull();
		expect(lstatSync(missing.legacyPath).isSymbolicLink()).toBe(true);

		const ambiguous = fixture();
		const ambiguousTombstone = join(ambiguous.layout.controlDir, "legacy-db-tombstone");
		const ambiguousRecoveryPath = join(
			ambiguous.layout.controlDir,
			"legacy-00000000-0000-4000-8000-000000000003.legacy-recovery.sqlite",
		);
		const conflictingRecoveryPath = join(
			ambiguous.layout.controlDir,
			"legacy-00000000-0000-4000-8000-000000000004.legacy-recovery.sqlite",
		);
		linkSync(ambiguous.legacyPath, ambiguousRecoveryPath);
		writeFileSync(conflictingRecoveryPath, "different database identity", { mode: 0o600 });
		mkdirSync(ambiguousTombstone, { recursive: true, mode: 0o700 });
		rmSync(ambiguous.legacyPath);
		symlinkSync(ambiguousTombstone, ambiguous.legacyPath, "dir");
		await expect(startDaemon({ dataDir: ambiguous.layout.dataDir })).rejects.toThrow(/ambiguous/i);
		expect(readCurrentDatabasePointer(ambiguous.layout)).toBeNull();
		expect(lstatSync(ambiguous.legacyPath).isSymbolicLink()).toBe(true);
		expect(existsSync(ambiguousRecoveryPath)).toBe(true);
		expect(existsSync(conflictingRecoveryPath)).toBe(true);
		const preservedRecovery = ReadOnlyActor.open(ambiguousRecoveryPath);
		try {
			expect(preservedRecovery.prepare("SELECT value FROM legacy_probe").pluck().get()).toBe(
				"preserved",
			);
		} finally {
			preservedRecovery.close();
		}
	}, 15_000);

	it("P1-T051-04-old-binary-split-brain", async () => {
		const { layout, legacyPath } = fixture();
		const legacySpool = join(layout.dataDir, "claude-hook-spool");
		const retryPath = join(legacySpool, "retry-event.json");
		mkdirSync(legacySpool, { recursive: true, mode: 0o700 });
		writeFileSync(
			join(legacySpool, "legacy-event.json"),
			JSON.stringify({
				hook_event_name: "UserPromptSubmit",
				session_id: "legacy-session",
				prompt: "legacy prompt",
				cwd: layout.dataDir,
			}),
			{ mode: 0o600 },
		);
		writeFileSync(retryPath, "{}\n", { mode: 0o600 });
		const daemon = await startDaemon({ dataDir: layout.dataDir });
		await daemon.stop();
		expect(existsSync(retryPath)).toBe(true);
		writeFileSync(
			retryPath,
			JSON.stringify({
				hook_event_name: "UserPromptSubmit",
				session_id: "legacy-session-retry",
				prompt: "legacy retry prompt",
				cwd: layout.dataDir,
			}),
			{ mode: 0o600 },
		);
		const restarted = await startDaemon({ dataDir: layout.dataDir });
		await restarted.stop();
		expect(existsSync(retryPath)).toBe(false);

		const attempt = spawnSync(
			process.execPath,
			[
				"-e",
				`const Database=require(${JSON.stringify(sqliteModule)}); const db=new Database(process.argv[1]); db.exec("CREATE TABLE split_brain(value TEXT)"); db.close();`,
				legacyPath,
			],
			{ encoding: "utf8" },
		);
		expect(attempt.status).not.toBe(0);
		expect(lstatSync(legacyPath).isSymbolicLink()).toBe(true);

		const pointer = readCurrentDatabasePointer(layout);
		expect(pointer).not.toBeNull();
		const restored = ReadOnlyActor.open(join(layout.dbDir, pointer ?? ""));
		try {
			expect(restored.prepare("SELECT value FROM legacy_probe").pluck().get()).toBe("preserved");
			expect(restored.prepare("SELECT COUNT(*) FROM raw_events").pluck().get()).toBe(2);
			expect(
				restored
					.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'split_brain'")
					.get(),
			).toBeUndefined();
		} finally {
			restored.close();
		}
		expect(existsSync(`${legacyPath}-wal`)).toBe(false);
		expect(existsSync(`${legacyPath}-shm`)).toBe(false);
		expect(existsSync(join(legacySpool, "legacy-event.json"))).toBe(false);

		const custom = fixture(true);
		const originalDb = process.env.CODEMEM_DB;
		process.env.CODEMEM_DB = custom.legacyPath;
		let customDaemon: Awaited<ReturnType<typeof startDaemon>> | undefined;
		try {
			customDaemon = await startDaemon({ dataDir: custom.layout.dataDir });
			await customDaemon.stop();
			const customPointer = readCurrentDatabasePointer(custom.layout);
			expect(customPointer).not.toBeNull();
			const customReader = ReadOnlyActor.open(join(custom.layout.dbDir, customPointer as string));
			try {
				expect(customReader.prepare("SELECT value FROM legacy_probe").pluck().get()).toBe(
					"preserved",
				);
			} finally {
				customReader.close();
			}
			expect(lstatSync(custom.legacyPath).isSymbolicLink()).toBe(true);
		} finally {
			await customDaemon?.stop().catch(() => {});
			if (originalDb === undefined) delete process.env.CODEMEM_DB;
			else process.env.CODEMEM_DB = originalDb;
		}

		const oldCustom = fixture(true, true);
		const oldDb = process.env.CODEMEM_DB;
		process.env.CODEMEM_DB = oldCustom.legacyPath;
		let oldDaemon: Awaited<ReturnType<typeof startDaemon>> | undefined;
		let resumedDaemon: Awaited<ReturnType<typeof startDaemon>> | undefined;
		try {
			oldDaemon = await startDaemon({ dataDir: oldCustom.layout.dataDir });
			await oldDaemon.stop();
			const resumedDataDir = resolveRuntimeDataDir({ dbPath: oldCustom.legacyPath });
			expect(resumedDataDir).toBe(oldCustom.layout.dataDir);
			resumedDaemon = await startDaemon({ dataDir: resumedDataDir });
			await resumedDaemon.stop();
			const resumedPointer = readCurrentDatabasePointer(oldCustom.layout);
			expect(resumedPointer).not.toBeNull();
			const resumedReader = ReadOnlyActor.open(
				join(oldCustom.layout.dbDir, resumedPointer as string),
			);
			try {
				expect(resumedReader.prepare("SELECT value FROM legacy_probe").pluck().get()).toBe(
					"preserved",
				);
			} finally {
				resumedReader.close();
			}
		} finally {
			await oldDaemon?.stop().catch(() => {});
			await resumedDaemon?.stop().catch(() => {});
			if (oldDb === undefined) delete process.env.CODEMEM_DB;
			else process.env.CODEMEM_DB = oldDb;
		}
	}, 15_000);
});
