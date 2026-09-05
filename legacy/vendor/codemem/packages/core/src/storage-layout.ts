import { createHash } from "node:crypto";
import { lstatSync, readlinkSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

export const DEFAULT_DATA_DIR = join(homedir(), ".codemem");
const DEFAULT_DB_PATH = join(DEFAULT_DATA_DIR, "mem.sqlite");

export interface StorageLayout {
	dataDir: string;
	controlDir: string;
	capabilitiesDir: string;
	capabilityManifestsDir: string;
	capabilityCurrentPointerPath: string;
	capabilityLifecycleLockPath: string;
	capabilityActivationReceiptPath: string;
	capabilitySetupTransactionPath: string;
	dbDir: string;
	versionsDir: string;
	currentPointerPath: string;
	journalPath: string;
	installManifestPath: string;
	lockPath: string;
	identityPath: string;
	socketPath: string;
	spoolDir: string;
	backupsDir: string;
}

export function resolveDatabaseRuntimeDataDir(dbPath: string): string {
	const resolvedDbPath = resolve(
		dbPath.startsWith("~/") ? join(homedir(), dbPath.slice(2)) : dbPath,
	);
	if (resolvedDbPath === resolve(DEFAULT_DB_PATH)) return DEFAULT_DATA_DIR;

	const legacyDataDir = dirname(resolvedDbPath);
	try {
		const tombstonePath = join(legacyDataDir, "control", "legacy-db-tombstone");
		if (
			lstatSync(resolvedDbPath).isSymbolicLink() &&
			resolve(dirname(resolvedDbPath), readlinkSync(resolvedDbPath)) === tombstonePath
		) {
			return legacyDataDir;
		}
	} catch {
		// Only the exact legacy tombstone target selects the old runtime root.
	}

	const runtimeId = createHash("sha256").update(resolvedDbPath, "utf8").digest("hex").slice(0, 32);
	return join(DEFAULT_DATA_DIR, "runtimes", runtimeId);
}

export function resolveRuntimeDataDir(options: { dataDir?: string; dbPath?: string } = {}): string {
	const dataDir = options.dataDir?.trim() || process.env.CODEMEM_DATA_DIR?.trim();
	if (dataDir) return dataDir;
	const dbPath = options.dbPath?.trim() || process.env.CODEMEM_DB?.trim();
	if (!dbPath) return DEFAULT_DATA_DIR;
	return resolveDatabaseRuntimeDataDir(dbPath);
}

export function resolveStorageLayout(dataDir: string = DEFAULT_DATA_DIR): StorageLayout {
	const root = resolve(dataDir);
	const controlDir = join(root, "control");
	const capabilitiesDir = join(controlDir, "capabilities");
	const dbDir = join(root, "db");
	return {
		dataDir: root,
		controlDir,
		capabilitiesDir,
		capabilityManifestsDir: join(capabilitiesDir, "manifests"),
		capabilityCurrentPointerPath: join(capabilitiesDir, "current"),
		capabilityLifecycleLockPath: join(capabilitiesDir, "lifecycle.lock"),
		capabilityActivationReceiptPath: join(capabilitiesDir, "activation-receipt.json"),
		capabilitySetupTransactionPath: join(capabilitiesDir, "setup-transaction.json"),
		dbDir,
		versionsDir: join(dbDir, "versions"),
		currentPointerPath: join(dbDir, "current"),
		journalPath: join(controlDir, "restore-journal.json"),
		installManifestPath: join(controlDir, "install-manifest.json"),
		lockPath: join(controlDir, "lock.db"),
		identityPath: join(controlDir, "identity.json"),
		socketPath: join(controlDir, "daemon.sock"),
		spoolDir: join(controlDir, "spool"),
		backupsDir: join(controlDir, "backups"),
	};
}
