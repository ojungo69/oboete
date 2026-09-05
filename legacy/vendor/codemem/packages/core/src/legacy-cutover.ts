import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
	chmodSync,
	existsSync,
	linkSync,
	lstatSync,
	readdirSync,
	readFileSync,
	readlinkSync,
	renameSync,
	statSync,
	symlinkSync,
	unlinkSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { assertInstallManifestCurrent } from "./install-manifest.js";
import { createOnlineBackup, requireVerifiedBackup, verifyOnlineBackup } from "./online-backup.js";
import {
	DEFAULT_DATA_DIR,
	readCurrentDatabasePointer,
	runLegacyMigration,
	type StorageLayout,
} from "./storage.js";
import { resolveDatabaseRuntimeDataDir } from "./storage-layout.js";
import {
	durableRemoveFile,
	durableReplaceFile,
	ensurePrivateDirectory,
	fsyncPath,
	readProcessIdentity,
} from "./storage-platform.js";
import { ReadOnlyActor, WriterActor } from "./writer-actor.js";

export type OpenFileOwner = { pid: number };
export type LegacyCutoverStep =
	| "prepared_owner_scan"
	| "tombstone_installed"
	| "final_owner_scan"
	| "legacy_handles_closed";

type FileIdentity = { dev: number; ino: number };

function fileIdentity(path: string): FileIdentity {
	const info = statSync(path);
	if (!info.isFile()) throw new Error("Legacy database path is not a regular file.");
	return { dev: info.dev, ino: info.ino };
}

function processUid(pid: number): number | null {
	try {
		const line = readFileSync(`/proc/${pid}/status`, "utf8")
			.split("\n")
			.find((value) => value.startsWith("Uid:"));
		if (!line) return null;
		const uid = Number.parseInt(line.split(/\s+/)[1] ?? "", 10);
		return Number.isSafeInteger(uid) ? uid : null;
	} catch {
		return null;
	}
}

function lsofOwners(path: string): Set<number> | null {
	const lsof = ["/usr/bin/lsof", "/usr/sbin/lsof"].find((candidate) => existsSync(candidate));
	if (!lsof) return null;
	const result = spawnSync(lsof, ["-nP", "-F", "p", "--", path], {
		encoding: "utf8",
		timeout: 2_000,
	});
	if (result.error || (result.status !== 0 && result.status !== 1)) return null;
	return new Set(
		(result.stdout ?? "")
			.split("\n")
			.filter((line) => /^p\d+$/.test(line))
			.map((line) => Number.parseInt(line.slice(1), 10)),
	);
}

function listOwnersByIdentity(identity: FileIdentity, evidencePath: string): OpenFileOwner[] {
	const owners = new Set<number>();
	const ownUid = process.getuid?.();
	let procIncomplete = false;
	for (const entry of readdirSync("/proc", { withFileTypes: true })) {
		if (!entry.isDirectory() || !/^\d+$/.test(entry.name)) continue;
		const pid = Number.parseInt(entry.name, 10);
		let descriptors: string[];
		try {
			descriptors = readdirSync(`/proc/${pid}/fd`);
		} catch (error) {
			const code = (error as NodeJS.ErrnoException).code;
			if (code === "ENOENT") continue;
			const uid = processUid(pid);
			if (ownUid !== undefined && uid !== null && uid !== ownUid) continue;
			procIncomplete = true;
			continue;
		}
		for (const descriptor of descriptors) {
			try {
				const info = statSync(`/proc/${pid}/fd/${descriptor}`);
				if (info.dev === identity.dev && info.ino === identity.ino) {
					owners.add(pid);
					break;
				}
			} catch {
				// A process may close a descriptor while /proc is being scanned.
			}
		}
	}
	const lsof = lsofOwners(evidencePath);
	if (procIncomplete && !lsof) {
		throw new Error("Legacy database owner scan was incomplete and trusted lsof is unavailable.");
	}
	for (const pid of lsof ?? []) owners.add(pid);
	return [...owners].sort((left, right) => left - right).map((pid) => ({ pid }));
}

export function listOpenFileOwners(path: string): OpenFileOwner[] {
	const info = statSync(path);
	if (!info.isFile()) return [];
	return listOwnersByIdentity({ dev: info.dev, ino: info.ino }, path);
}

function sameProcessIdentity(
	left: { startTime: string; fingerprint: string },
	right: { startTime: string; fingerprint: string },
): boolean {
	return left.startTime === right.startTime && left.fingerprint === right.fingerprint;
}

function isTrustedLegacyCodememOwner(pid: number): boolean {
	let command: string;
	try {
		command = readFileSync(`/proc/${pid}/cmdline`, "utf8").replaceAll("\0", " ").toLowerCase();
	} catch {
		return false;
	}
	const runtime =
		/(^|[\s/])codemem(?:\.py)?([\s]|$)/.test(command) ||
		command.includes("/packages/cli/dist/index.js") ||
		command.includes("/packages/cli/src/index.ts");
	return runtime && /\b(serve|daemon|mcp|hook|ingest|inject)\b/.test(command);
}

async function waitForOwnersToClear(
	identity: FileIdentity,
	evidencePath: string,
	timeoutMs: number,
): Promise<boolean> {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		if (listOwnersByIdentity(identity, evidencePath).length === 0) return true;
		await new Promise((resolveWait) => setTimeout(resolveWait, 20));
	}
	return listOwnersByIdentity(identity, evidencePath).length === 0;
}

async function requestLegacyOwnersStop(
	identity: FileIdentity,
	evidencePath: string,
): Promise<void> {
	if (await waitForOwnersToClear(identity, evidencePath, 250)) return;
	for (const { pid } of listOwnersByIdentity(identity, evidencePath)) {
		if (pid === process.pid || !isTrustedLegacyCodememOwner(pid)) {
			throw new Error(`Legacy database open handle remains owned by pid ${pid}.`);
		}
		const before = readProcessIdentity(pid);
		if (!listOwnersByIdentity(identity, evidencePath).some((owner) => owner.pid === pid)) {
			continue;
		}
		const confirmed = readProcessIdentity(pid);
		if (!sameProcessIdentity(before, confirmed)) {
			throw new Error(`Legacy database owner identity changed for pid ${pid}.`);
		}
		process.kill(pid, "SIGTERM");
	}
	if (!(await waitForOwnersToClear(identity, evidencePath, 2_000))) {
		throw new Error("Legacy database owner did not stop; cutover was not started.");
	}
}

function assertSoleCutoverOwner(identity: FileIdentity, evidencePath: string): void {
	const owners = listOwnersByIdentity(identity, evidencePath);
	if (owners.length !== 1 || owners[0]?.pid !== process.pid) {
		throw new Error(
			`Legacy database owner set must contain only the cutover daemon; got ${
				owners.map((owner) => owner.pid).join(",") || "none"
			}.`,
		);
	}
}

function recordCutoverStep(
	onStep: ((event: LegacyCutoverStep) => void) | undefined,
	event: LegacyCutoverStep,
): void {
	onStep?.(event);
}

function installTombstone(layout: StorageLayout, legacyPath: string): void {
	const tombstoneDir = join(layout.controlDir, "legacy-db-tombstone");
	ensurePrivateDirectory(tombstoneDir);
	durableReplaceFile(
		join(tombstoneDir, "README"),
		"This path fences the retired pre-daemon database layout.\n",
	);
	const temporaryPath = `${legacyPath}.${process.pid}.${randomUUID()}.tombstone`;
	try {
		symlinkSync(tombstoneDir, temporaryPath, "dir");
		renameSync(temporaryPath, legacyPath);
		fsyncPath(dirname(legacyPath));
	} catch (error) {
		try {
			unlinkSync(temporaryPath);
		} catch {
			// The temporary link may already have been renamed.
		}
		throw error;
	}
}

function createRecoveryLink(
	layout: StorageLayout,
	legacyPath: string,
	operationId: string,
): string {
	const recoveryPath = join(layout.controlDir, `${operationId}.legacy-recovery.sqlite`);
	linkSync(legacyPath, recoveryPath);
	chmodSync(recoveryPath, 0o600);
	fsyncPath(recoveryPath);
	fsyncPath(layout.controlDir);
	return recoveryPath;
}

function restoreRecoveryLink(legacyPath: string, recoveryPath: string): void {
	renameSync(recoveryPath, legacyPath);
	fsyncPath(dirname(legacyPath));
}

const LEGACY_RECOVERY_NAME =
	/^legacy-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.legacy-recovery\.sqlite$/;

function recoverInterruptedLegacyCutover(layout: StorageLayout, legacyPath: string): void {
	const legacyInfo = lstatSync(legacyPath);
	const tombstoned =
		legacyInfo.isSymbolicLink() &&
		resolve(dirname(legacyPath), readlinkSync(legacyPath)) ===
			join(layout.controlDir, "legacy-db-tombstone");
	if (!tombstoned && !legacyInfo.isFile()) return;

	const recoveryPaths = readdirSync(layout.controlDir)
		.filter((name) => LEGACY_RECOVERY_NAME.test(name))
		.map((name) => join(layout.controlDir, name))
		.filter((path) => lstatSync(path).isFile())
		.sort();
	if (recoveryPaths.length === 0) {
		if (tombstoned) throw new Error("Interrupted legacy cutover recovery file is missing.");
		return;
	}

	const identity = fileIdentity(tombstoned ? (recoveryPaths[0] as string) : legacyPath);
	if (!recoveryPaths.every((path) => pathHasIdentity(path, identity))) {
		throw new Error("Interrupted legacy cutover recovery files are ambiguous.");
	}
	if (tombstoned) restoreRecoveryLink(legacyPath, recoveryPaths.shift() as string);
	for (const path of recoveryPaths) durableRemoveFile(path);
}

function pathHasIdentity(path: string, identity: FileIdentity): boolean {
	try {
		const current = statSync(path);
		return current.isFile() && current.dev === identity.dev && current.ino === identity.ino;
	} catch {
		return false;
	}
}

function rollbackPublishedArtifact(
	layout: StorageLayout,
	pointer: string,
	artifactPath: string,
): void {
	if (readCurrentDatabasePointer(layout) !== pointer) {
		throw new Error("Legacy cutover rollback found an unexpected current database pointer.");
	}
	durableRemoveFile(layout.currentPointerPath);
	durableRemoveFile(artifactPath);
}

export async function cutoverLegacyDatabase(input: {
	layout: StorageLayout;
	legacyPath: string;
	onStep?: (event: LegacyCutoverStep) => void;
}): Promise<{ operationId: string; pointer: string }> {
	if (readCurrentDatabasePointer(input.layout) !== null) {
		throw new Error("Legacy cutover refuses to replace an existing canonical database.");
	}
	const legacyInfo = lstatSync(input.legacyPath);
	if (legacyInfo.isSymbolicLink() || !legacyInfo.isFile()) {
		throw new Error("Legacy database path is not a regular file; cutover stopped.");
	}
	assertInstallManifestCurrent(input.layout.installManifestPath);

	const identity = fileIdentity(input.legacyPath);
	await requestLegacyOwnersStop(identity, input.legacyPath);
	const operationId = `legacy-${randomUUID()}`;
	const lock = WriterActor.open(input.legacyPath);
	let reader: ReadOnlyActor | null = null;
	let transactionOpen = false;
	let recoveryPath: string | null = null;
	let published: { pointer: string; artifactPath: string } | null = null;
	let tombstoneInstalled = false;
	let completed = false;
	try {
		lock.pragma("busy_timeout = 0");
		const journalMode = String(lock.pragma("journal_mode = WAL", { simple: true })).toLowerCase();
		if (journalMode !== "wal") throw new Error("Legacy database could not enter WAL mode.");
		reader = ReadOnlyActor.open(input.legacyPath);
		lock.exec("BEGIN EXCLUSIVE");
		transactionOpen = true;
		assertSoleCutoverOwner(identity, input.legacyPath);
		recordCutoverStep(input.onStep, "prepared_owner_scan");
		recoveryPath = createRecoveryLink(input.layout, input.legacyPath, operationId);

		const proof = await createOnlineBackup({
			db: reader,
			destinationDir: input.layout.backupsDir,
			operationId,
			reason: "legacy-cutover",
		});
		requireVerifiedBackup(proof);
		const check = verifyOnlineBackup({
			artifactPath: proof.artifactPath,
			expectedSha256: proof.artifactSha256,
			sourcePath: input.legacyPath,
		});
		if (!check.valid) throw new Error("Legacy cutover backup verification failed.");

		if (!pathHasIdentity(input.legacyPath, identity)) {
			throw new Error("Legacy database path changed during cutover.");
		}
		installTombstone(input.layout, input.legacyPath);
		tombstoneInstalled = true;
		recordCutoverStep(input.onStep, "tombstone_installed");
		assertSoleCutoverOwner(identity, recoveryPath);
		recordCutoverStep(input.onStep, "final_owner_scan");
		assertInstallManifestCurrent(input.layout.installManifestPath);

		const pointer = `versions/${operationId}.sqlite`;
		const artifactPath = join(input.layout.dbDir, pointer);
		runLegacyMigration({
			layout: input.layout,
			operationId,
			verifiedBackupPath: proof.artifactPath,
			verifiedBackupSha256: proof.artifactSha256,
		});
		published = { pointer, artifactPath };

		lock.exec("COMMIT");
		transactionOpen = false;
		reader.close();
		reader = null;
		lock.close();
		recordCutoverStep(input.onStep, "legacy_handles_closed");
		completed = true;

		for (const path of [recoveryPath, `${input.legacyPath}-wal`, `${input.legacyPath}-shm`]) {
			if (!path) continue;
			try {
				durableRemoveFile(path);
			} catch {
				console.error("[codemem] legacy cutover committed; private cleanup remains pending.");
			}
		}
		return { operationId, pointer };
	} catch (error) {
		const recoveryErrors: unknown[] = [];
		if (
			recoveryPath &&
			existsSync(recoveryPath) &&
			(tombstoneInstalled || !pathHasIdentity(input.legacyPath, identity))
		) {
			try {
				restoreRecoveryLink(input.legacyPath, recoveryPath);
				recoveryPath = null;
			} catch (recoveryError) {
				recoveryErrors.push(recoveryError);
			}
		}
		if (recoveryPath && existsSync(recoveryPath)) {
			try {
				durableRemoveFile(recoveryPath);
				recoveryPath = null;
			} catch (recoveryError) {
				recoveryErrors.push(recoveryError);
			}
		}
		if (published) {
			try {
				rollbackPublishedArtifact(input.layout, published.pointer, published.artifactPath);
			} catch (recoveryError) {
				recoveryErrors.push(recoveryError);
			}
		}
		if (recoveryErrors.length > 0) {
			throw new AggregateError(
				[error, ...recoveryErrors],
				"Legacy cutover failed and rollback did not complete.",
			);
		}
		throw error;
	} finally {
		if (!completed) {
			if (transactionOpen) {
				try {
					lock.exec("ROLLBACK");
				} catch {
					// Closing the actor below releases any surviving transaction.
				}
			}
			try {
				reader?.close();
			} catch {
				// Keep the cutover failure.
			}
			try {
				if (lock.open) lock.close();
			} catch {
				// Keep the cutover failure.
			}
		}
	}
}

function expandHome(path: string): string {
	return path.startsWith("~/") ? join(homedir(), path.slice(2)) : path;
}

export async function cutoverLegacyLayoutIfNeeded(
	layout: StorageLayout,
): Promise<{ operationId: string; pointer: string } | null> {
	if (readCurrentDatabasePointer(layout) !== null) return null;
	const configured = process.env.CODEMEM_DB?.trim();
	const candidates = new Set<string>();
	if (configured) {
		const path = resolve(expandHome(configured));
		if (
			existsSync(path) &&
			(dirname(path) === layout.dataDir || resolveDatabaseRuntimeDataDir(path) === layout.dataDir)
		) {
			candidates.add(path);
		}
	}
	if (candidates.size === 0) {
		const defaultLegacy = join(layout.dataDir, "mem.sqlite");
		if (existsSync(defaultLegacy)) candidates.add(defaultLegacy);
		if (layout.dataDir === resolve(DEFAULT_DATA_DIR)) {
			for (const path of [
				join(homedir(), ".codemem.sqlite"),
				join(homedir(), ".opencode-mem.sqlite"),
			]) {
				if (existsSync(path)) candidates.add(path);
			}
		}
	}
	if (candidates.size === 0) return null;
	if (candidates.size > 1) {
		throw new Error(
			"Multiple legacy database paths exist; cutover requires one unambiguous source.",
		);
	}
	const legacyPath = [...candidates][0] as string;
	recoverInterruptedLegacyCutover(layout, legacyPath);
	return cutoverLegacyDatabase({ layout, legacyPath });
}
