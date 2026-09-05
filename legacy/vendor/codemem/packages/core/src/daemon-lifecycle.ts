import { randomUUID } from "node:crypto";
import { appendFileSync, chmodSync, existsSync, readFileSync, unlinkSync } from "node:fs";
import { createConnection, createServer, type Server } from "node:net";
import { resolve } from "node:path";
import BetterSqlite3 from "better-sqlite3";
import {
	captureOnlyCapabilityProjection,
	ProviderTlsPreflightError,
	preflightProviderTls,
	providerTransportProfile,
	safeManifestProjection,
} from "./capability-manifest.js";
import { type CanonicalWriter, openCanonicalWriter } from "./daemon-canonical.js";
import { DaemonJobService } from "./daemon-jobs.js";
import { DaemonOperationService, recoverDaemonRestoresBeforeOpen } from "./daemon-operations.js";
import { attachDaemonRpc, type DaemonRpcContext, dispatchSpoolMutation } from "./daemon-rpc.js";
import { compileProviderDestinationBoundary } from "./destination-boundary.js";
import { cutoverLegacyLayoutIfNeeded } from "./legacy-cutover.js";
import {
	buildNormalizedEventFromClaudeHook,
	buildNormalizedEventFromCodexHook,
} from "./normalized-event.js";
import { ObserverClient } from "./observer-client.js";
import { createDailyBackup } from "./online-backup.js";
import { RawEventSweeper } from "./raw-event-sweeper.js";
import { warmRedactionWorker } from "./redaction-worker.js";
import {
	acquireSpoolLock,
	drainLegacySpool,
	importReadySpoolEntries,
	spoolMutation,
} from "./spool.js";
import {
	acquireCapabilityLifecycleLock,
	ensureStorageLayout,
	readCurrentCapabilityManifest,
	readValidatedCapabilityActivationReceipt,
	recoverCapabilitySetupTransaction,
	recoverStorageJournal,
	resolveStorageLayout,
	type StorageLayout,
} from "./storage.js";
import {
	assertDataDirPreflight,
	assertSupportedStoragePlatform,
	durableRemoveFile,
	durableReplaceFile,
	readProcessIdentity,
} from "./storage-platform.js";
import type { MemoryStore } from "./store.js";
import { ViewerAuthState } from "./viewer-auth.js";
import { createViewerReadHandler } from "./viewer-read.js";
import type { WriterActor } from "./writer-actor.js";

export type DaemonIdentity = {
	version: 1;
	pid: number;
	startTime: string;
	fingerprint: string;
	nonce: string;
};

export type DaemonHealth =
	| { status: "ok"; pid: number; socketPath: string; dataDir: string }
	| { status: "not_running"; dataDir: string };

export type DaemonHandle = {
	dataDir: string;
	layout: StorageLayout;
	identity: DaemonIdentity;
	lockPath: string;
	socketPath: string;
	identityPath: string;
	capability: DaemonCapabilityState;
	stop(): Promise<void>;
};

export type DaemonCapabilityState =
	| ReturnType<typeof captureOnlyCapabilityProjection>
	| ReturnType<typeof safeManifestProjection>;

type LiveDaemon = {
	lock: BetterSqlite3.Database;
	server: Server;
	identity: DaemonIdentity;
	layout: StorageLayout;
	writer: WriterActor;
	store: MemoryStore;
	sweeper: RawEventSweeper | null;
	jobs: DaemonJobService;
	spoolSweepTimer: ReturnType<typeof setInterval>;
	backupSweepTimer: ReturnType<typeof setInterval> | null;
	dailyBackupTask: Promise<void> | null;
	lastDailyBackupId: string | null;
};

const liveDaemons = new Map<string, LiveDaemon>();

function recordLockOpen(dbPath: string): void {
	const tracePath = process.env.CODEMEM_DB_OPEN_TRACE?.trim();
	if (!tracePath) return;
	appendFileSync(
		tracePath,
		`${JSON.stringify({
			version: 1,
			event: "sqlite_open",
			mode: "lock",
			owner: "daemon_lifecycle",
			pid: process.pid,
			dbPath: resolve(dbPath),
			openedAt: new Date().toISOString(),
		})}\n`,
		{ encoding: "utf8", mode: 0o600 },
	);
}

function readIdentityFile(path: string): DaemonIdentity | null {
	if (!existsSync(path)) return null;
	const parsed = JSON.parse(readFileSync(path, "utf8")) as Partial<DaemonIdentity>;
	if (
		parsed.version !== 1 ||
		typeof parsed.pid !== "number" ||
		typeof parsed.startTime !== "string" ||
		typeof parsed.fingerprint !== "string" ||
		typeof parsed.nonce !== "string"
	) {
		throw new Error("Daemon identity record is malformed.");
	}
	return parsed as DaemonIdentity;
}

function identitiesMatch(
	file: DaemonIdentity,
	live: { startTime: string; fingerprint: string },
): boolean {
	return file.startTime === live.startTime && file.fingerprint === live.fingerprint;
}

function sameIdentity(left: DaemonIdentity, right: DaemonIdentity): boolean {
	return (
		left.pid === right.pid &&
		left.startTime === right.startTime &&
		left.fingerprint === right.fingerprint &&
		left.nonce === right.nonce
	);
}

function processAlive(pid: number): boolean {
	try {
		const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
		const closeParen = stat.lastIndexOf(")");
		const state = stat.slice(closeParen + 2).split(" ")[0];
		return state !== "Z" && state !== "X";
	} catch {
		return false;
	}
}

async function waitUntil(predicate: () => boolean, timeoutMs: number): Promise<boolean> {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		if (predicate()) return true;
		const remaining = deadline - Date.now();
		if (remaining <= 0) break;
		await new Promise((resolve) => setTimeout(resolve, Math.min(20, remaining)));
	}
	return predicate();
}

function isDaemonProcessAlive(layout: StorageLayout): boolean {
	let identity: DaemonIdentity | null;
	try {
		identity = readIdentityFile(layout.identityPath);
	} catch {
		return false;
	}
	if (!identity) return false;
	if (!processAlive(identity.pid)) return false;
	try {
		return identitiesMatch(identity, readProcessIdentity(identity.pid));
	} catch {
		return false;
	}
}

function acquireWriterLock(lockPath: string): BetterSqlite3.Database {
	const lock = new BetterSqlite3(lockPath, { timeout: 0, fileMustExist: false });
	try {
		chmodSync(lockPath, 0o600);
		lock.pragma("journal_mode = DELETE");
		lock.pragma("busy_timeout = 0");
		lock.exec("BEGIN IMMEDIATE");
		recordLockOpen(lockPath);
		return lock;
	} catch (error) {
		lock.close();
		const message = error instanceof Error ? error.message : String(error);
		if (/busy|locked|SQLITE_BUSY/i.test(message)) {
			throw new Error("Daemon already running for this data_dir (writer lock busy).");
		}
		throw error;
	}
}

export function probeDaemonWriterAvailable(dataDir: string): boolean {
	let lease: ReturnType<typeof acquireDaemonWriterLease>;
	try {
		lease = acquireDaemonWriterLease(dataDir);
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		if (/already running|busy|locked|SQLITE_BUSY/i.test(message)) return false;
		throw error;
	}
	lease.close();
	return true;
}

export function acquireDaemonWriterLease(dataDir: string): { close(): void } {
	const layout = resolveStorageLayout(dataDir);
	ensureStorageLayout(layout);
	const lock = acquireWriterLock(layout.lockPath);
	return {
		close(): void {
			if (lock.open) lock.close();
		},
	};
}

function bindPrivateSocket(socketPath: string, rpc: DaemonRpcContext): Promise<Server> {
	if (existsSync(socketPath)) unlinkSync(socketPath);
	const server = createServer((connection) => {
		attachDaemonRpc(connection, rpc);
	});
	return new Promise((resolveListen, reject) => {
		server.once("error", reject);
		server.listen(socketPath, () => {
			try {
				chmodSync(socketPath, 0o600);
				resolveListen(server);
			} catch (error) {
				server.close();
				reject(error);
			}
		});
	});
}

async function releaseResources(layout: StorageLayout, live?: LiveDaemon): Promise<void> {
	if (live) {
		clearInterval(live.spoolSweepTimer);
		if (live.backupSweepTimer) clearInterval(live.backupSweepTimer);
		await live.dailyBackupTask;
		try {
			live.server.close();
		} catch {
			// server may already be closed
		}
		await live.jobs.stop();
		await live.sweeper?.stop();
		try {
			live.store.close();
		} catch {
			// store may already be closed
		}
		try {
			if (live.writer.open) live.writer.close();
		} catch {
			// writer is released with the daemon
		}
	}
	removeControlArtifacts(layout);
	if (live) {
		try {
			if (live.lock.open) live.lock.close();
		} catch {
			// lock is released after control artifacts are gone
		}
	}
}

async function stopLive(dataDir: string): Promise<void> {
	const live = liveDaemons.get(dataDir);
	if (!live) return;
	liveDaemons.delete(dataDir);
	await releaseResources(live.layout, live);
}

function requestCleanStop(socketPath: string, nonce: string): Promise<boolean> {
	return new Promise((resolveStop) => {
		const socket = createConnection(socketPath);
		let settled = false;
		const finish = (ok: boolean) => {
			if (settled) return;
			settled = true;
			socket.destroy();
			resolveStop(ok);
		};
		socket.setTimeout(500);
		socket.once("connect", () => {
			socket.write(`STOP ${nonce}\n`);
			socket.end();
		});
		socket.once("error", () => finish(false));
		socket.once("timeout", () => finish(false));
		socket.once("close", () => finish(true));
	});
}

export async function startDaemon(options: {
	dataDir: string;
	rpcDeadlineMs?: number;
	now?: () => number;
}): Promise<DaemonHandle> {
	assertSupportedStoragePlatform();
	assertDataDirPreflight(options.dataDir);
	const layout = resolveStorageLayout(options.dataDir);
	ensureStorageLayout(layout);
	warmRedactionWorker();
	const lifecycle = acquireCapabilityLifecycleLock(layout);
	let setupLock: ReturnType<typeof acquireSpoolLock> | undefined;
	let lock: BetterSqlite3.Database;
	try {
		setupLock = acquireSpoolLock(layout.dataDir);
		lock = acquireWriterLock(layout.lockPath);
	} catch (error) {
		setupLock?.close();
		lifecycle.close();
		throw error;
	}

	let canonical: CanonicalWriter | undefined;
	let sweeper: RawEventSweeper | null = null;
	let jobs: DaemonJobService | undefined;
	let server: Server | undefined;
	let live: LiveDaemon | undefined;
	let started = false;
	try {
		recoverCapabilitySetupTransaction(layout, lifecycle);
		setupLock?.close();
		setupLock = undefined;
		recoverStorageJournal(layout);
		const manifest = readCurrentCapabilityManifest(layout);
		let validatedActivationReceipt: ReturnType<typeof readValidatedCapabilityActivationReceipt> =
			null;
		let providerHealth: "available" | "provider_unavailable" | "provider_tls_rejected" =
			"available";
		let providerTlsPeerVerified = false;
		let activationReceiptState: "absent" | "validated" | "rejected" = "absent";
		let capability: DaemonCapabilityState;
		if (manifest) {
			try {
				validatedActivationReceipt = readValidatedCapabilityActivationReceipt(layout, manifest);
				if (validatedActivationReceipt) {
					activationReceiptState = "validated";
				}
			} catch {
				activationReceiptState = "rejected";
				console.error("[codemem] capability activation receipt was rejected.");
			}
			try {
				const preflight = await preflightProviderTls(manifest.summaryProvider);
				providerTlsPeerVerified = !("skipped" in preflight);
			} catch (error) {
				providerHealth =
					error instanceof ProviderTlsPreflightError ? error.reason : "provider_unavailable";
			}
		}
		await cutoverLegacyLayoutIfNeeded(layout);
		recoverDaemonRestoresBeforeOpen(layout.dataDir);
		canonical = await openCanonicalWriter(layout);
		if (manifest) {
			if (validatedActivationReceipt?.version === 2) {
				canonical.store.importActivationReceipt({
					receiptId: validatedActivationReceipt.receiptId,
					activationSequence: validatedActivationReceipt.activationSequence,
					manifestFingerprint: manifest.configurationFingerprint,
					providerFingerprint: manifest.summaryProvider.providerFingerprint,
				});
			}
			canonical.store.recordProviderHealth({
				manifestFingerprint: manifest.configurationFingerprint,
				providerFingerprint: manifest.summaryProvider.providerFingerprint,
				health: providerHealth,
			});
			capability = safeManifestProjection(
				manifest,
				providerHealth,
				activationReceiptState,
				"ready",
			);
		} else {
			capability = captureOnlyCapabilityProjection("ready");
		}
		const liveIdentity = readProcessIdentity(process.pid);
		const identity: DaemonIdentity = {
			version: 1,
			pid: process.pid,
			startTime: liveIdentity.startTime,
			fingerprint: liveIdentity.fingerprint,
			nonce: randomUUID(),
		};
		let observer: ObserverClient | null = null;
		let providerDestinationBoundary:
			| ReturnType<typeof compileProviderDestinationBoundary>
			| undefined;
		if (manifest && capability.providerEnabled) {
			providerDestinationBoundary = compileProviderDestinationBoundary(manifest, {
				repositoryIdentity: null,
				tlsPeerVerified: providerTlsPeerVerified,
			});
			observer = new ObserverClient(
				manifest.summaryProvider,
				providerTransportProfile(manifest.resourceProfile),
				{
					destinationBoundary: providerDestinationBoundary,
					onHealthState: async (state) => {
						canonical?.store.recordProviderHealth({
							manifestFingerprint: manifest.configurationFingerprint,
							providerFingerprint: manifest.summaryProvider.providerFingerprint,
							health: state === "healthy" ? "available" : "provider_unavailable",
						});
					},
				},
			);
			sweeper = new RawEventSweeper(canonical.store, {
				observer,
				resourceProfile: manifest.resourceProfile,
				capabilityManifest: manifest,
				configurationFingerprint: manifest.configurationFingerprint,
				providerFingerprint: manifest.summaryProvider.providerFingerprint,
				providerTlsPeerVerified,
			});
		}
		jobs = new DaemonJobService(canonical.store, {
			dataDir: layout.dataDir,
			capability: manifest
				? { ...capability, destinationBoundary: providerDestinationBoundary }
				: capability,
		});
		const operations = new DaemonOperationService(canonical.store, jobs, layout.dataDir);
		const rpc: DaemonRpcContext = {
			identity,
			dataDir: layout.dataDir,
			deadlineMs: options.rpcDeadlineMs,
			now: options.now,
			writer: canonical.db,
			store: canonical.store,
			viewerAuth: new ViewerAuthState({
				controlDir: layout.controlDir,
				instanceId: identity.nonce,
				now: options.now,
			}),
			viewerRead: createViewerReadHandler({
				store: canonical.store,
				sweeper,
				observer,
				capability,
			}),
			jobs,
			operations,
			capability,
			restoreState: { active: false },
			onStop: () => {
				const current = liveDaemons.get(layout.dataDir);
				if (current && sameIdentity(current.identity, identity)) {
					void stopLive(layout.dataDir).catch(() => {
						console.error("[codemem] daemon shutdown failed.");
					});
				}
			},
		};
		const sweepSpool = () => {
			if (jobs?.isMaintenanceMode() || rpc.restoreState?.active) return;
			try {
				importReadySpoolEntries(layout.dataDir, (entry) => dispatchSpoolMutation(rpc, entry));
			} catch {
				console.error("[codemem] spool sweep failed; ready entries were retained.");
			}
		};
		const drainSpoolAfterStartup = async () => {
			const drained = await drainLegacySpool(layout.dataDir, (source, payload) => {
				const event =
					source === "claude"
						? buildNormalizedEventFromClaudeHook(payload)
						: buildNormalizedEventFromCodexHook(payload);
				if (!event) return false;
				const idempotencyKey = String(event.idempotencyKey);
				return (
					spoolMutation(
						{
							method: "POST /v1/events",
							idempotencyKey,
							body: { idempotencyKey, event },
						},
						{ dataDir: layout.dataDir },
					).status !== "dropped"
				);
			});
			if (drained.failed > 0) {
				console.error("[codemem] some legacy spool entries were retained for retry.");
			}
			sweepSpool();
		};
		try {
			await drainSpoolAfterStartup();
		} catch {
			console.error("[codemem] startup spool drain failed; entries were retained.");
		}
		server = await bindPrivateSocket(layout.socketPath, rpc);
		durableReplaceFile(layout.identityPath, `${JSON.stringify(identity)}\n`);
		jobs.startInternalBackfills();
		const spoolSweepTimer = setInterval(sweepSpool, 1_000);
		spoolSweepTimer.unref();
		const startingLive: LiveDaemon = {
			lock,
			server,
			identity,
			layout,
			writer: canonical.db,
			store: canonical.store,
			sweeper,
			jobs,
			spoolSweepTimer,
			backupSweepTimer: null,
			dailyBackupTask: null,
			lastDailyBackupId: null,
		};
		live = startingLive;
		const sweepBackup = () => {
			if (jobs?.isMaintenanceMode() || rpc.restoreState?.active) return;
			const instant = new Date((options.now ?? Date.now)());
			const backupId = `daily-${instant.toISOString().slice(0, 10)}`;
			if (startingLive.dailyBackupTask || startingLive.lastDailyBackupId === backupId) return;
			startingLive.dailyBackupTask = createDailyBackup({
				db: startingLive.writer,
				destinationDir: startingLive.layout.backupsDir,
				now: () => instant,
			})
				.then((proof) => {
					startingLive.lastDailyBackupId = proof.backupId;
				})
				.catch(() => {
					console.error("[codemem] daily backup failed; it will be retried.");
				})
				.finally(() => {
					startingLive.dailyBackupTask = null;
				});
		};
		startingLive.backupSweepTimer = setInterval(sweepBackup, 60_000);
		startingLive.backupSweepTimer.unref();
		const handle: DaemonHandle = {
			dataDir: layout.dataDir,
			layout,
			identity,
			lockPath: layout.lockPath,
			socketPath: layout.socketPath,
			identityPath: layout.identityPath,
			capability,
			stop: async () => {
				const current = liveDaemons.get(layout.dataDir);
				if (!current || !sameIdentity(current.identity, identity)) return;
				await stopLive(layout.dataDir);
			},
		};
		liveDaemons.set(layout.dataDir, startingLive);
		sweeper?.start();
		lifecycle.close();
		started = true;
		return handle;
	} catch (error) {
		if (live) {
			if (liveDaemons.get(layout.dataDir) === live) liveDaemons.delete(layout.dataDir);
			clearInterval(live.spoolSweepTimer);
			if (live.backupSweepTimer) clearInterval(live.backupSweepTimer);
		}
		if (jobs) await jobs.stop();
		if (canonical) {
			try {
				canonical.store.close();
			} catch {
				// store may already be closed
			}
			try {
				if (canonical.db.open) canonical.db.close();
			} catch {
				// writer is released with the failed start
			}
		}
		if (server) {
			try {
				server.close();
			} catch {
				// bind succeeded but later startup failed
			}
		}
		if (existsSync(layout.socketPath)) {
			try {
				unlinkSync(layout.socketPath);
			} catch {
				// leftover socket must not outlive a failed start
			}
		}
		try {
			durableRemoveFile(layout.identityPath);
		} catch {
			// keep the original startup error
		}
		throw error;
	} finally {
		if (!started) {
			try {
				if (lock.open) lock.close();
			} catch {
				// lock must not leak if startup fails after acquire
			}
		}
		if (!started) {
			setupLock?.close();
			lifecycle.close();
		}
	}
}

function removeControlArtifacts(layout: StorageLayout): void {
	if (existsSync(layout.socketPath)) {
		try {
			unlinkSync(layout.socketPath);
		} catch {
			// stale socket cleanup is best-effort
		}
	}
	durableRemoveFile(layout.identityPath);
}

function cleanupIfStillOwner(layout: StorageLayout, snapshot: DaemonIdentity | null): void {
	let lock: BetterSqlite3.Database;
	try {
		lock = acquireWriterLock(layout.lockPath);
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		if (/already running/i.test(message)) return;
		throw error;
	}
	try {
		const current = readIdentityFile(layout.identityPath);
		if (snapshot && current && !sameIdentity(snapshot, current)) return;
		removeControlArtifacts(layout);
	} finally {
		lock.close();
	}
}

export function readDaemonHealth(dataDir: string): DaemonHealth {
	const layout = resolveStorageLayout(dataDir);
	if (!existsSync(layout.socketPath) || !isDaemonProcessAlive(layout)) {
		return { status: "not_running", dataDir: layout.dataDir };
	}
	let identity: DaemonIdentity | null;
	try {
		identity = readIdentityFile(layout.identityPath);
	} catch {
		return { status: "not_running", dataDir: layout.dataDir };
	}
	if (!identity) return { status: "not_running", dataDir: layout.dataDir };
	return {
		status: "ok",
		pid: identity.pid,
		socketPath: layout.socketPath,
		dataDir: layout.dataDir,
	};
}

export async function forceKillDaemon(dataDir: string, expected?: DaemonIdentity): Promise<void> {
	const layout = resolveStorageLayout(dataDir);
	const first = readIdentityFile(layout.identityPath);
	if (!first) throw new Error("Force-kill refused: no daemon identity record.");
	if (expected && !sameIdentity(expected, first)) {
		throw new Error("Force-kill refused: daemon identity mismatch.");
	}
	let liveFirst: { startTime: string; fingerprint: string };
	try {
		liveFirst = readProcessIdentity(first.pid);
	} catch {
		throw new Error("Force-kill refused: daemon identity mismatch.");
	}
	if (!identitiesMatch(first, liveFirst)) {
		throw new Error("Force-kill refused: daemon identity mismatch.");
	}
	const second = readIdentityFile(layout.identityPath);
	if (!second || !sameIdentity(first, second)) {
		throw new Error("Force-kill refused: daemon identity mismatch.");
	}
	let liveSecond: { startTime: string; fingerprint: string };
	try {
		liveSecond = readProcessIdentity(second.pid);
	} catch {
		throw new Error("Force-kill refused: daemon identity mismatch.");
	}
	if (!identitiesMatch(second, liveSecond)) {
		throw new Error("Force-kill refused: daemon identity mismatch.");
	}
	if (second.pid === process.pid) {
		throw new Error("Force-kill refused: the daemon runs in the calling process; use stopDaemon.");
	}

	// ponytail: kill(2) still addresses a PID number. pidfd_send_signal would
	// close the reuse window after this last check; Node has no pidfd yet.
	// Upgrade: pidfd_open + pidfd_send_signal when available. T055 covers reuse.
	process.kill(second.pid, "SIGKILL");
	if (!(await waitUntil(() => !processAlive(second.pid), 1000))) {
		throw new Error("Force-kill did not terminate the identified process.");
	}
	const live = liveDaemons.get(layout.dataDir);
	if (live && sameIdentity(live.identity, second)) {
		liveDaemons.delete(layout.dataDir);
		try {
			if (live.lock.open) live.lock.close();
		} catch {
			// lock is released with the dead owner
		}
	}
	cleanupIfStillOwner(layout, second);
}

export async function stopDaemon(
	dataDir: string,
	options?: { timeoutMs?: number },
): Promise<{ action: "stopped" | "force_killed" }> {
	const layout = resolveStorageLayout(dataDir);
	const snapshot = readIdentityFile(layout.identityPath);
	if (liveDaemons.has(layout.dataDir)) {
		await stopLive(layout.dataDir);
		return { action: "stopped" };
	}
	const timeoutMs = options?.timeoutMs ?? 2000;
	if (snapshot && existsSync(layout.socketPath)) {
		await requestCleanStop(layout.socketPath, snapshot.nonce);
	}
	if (await waitUntil(() => !isDaemonProcessAlive(layout), timeoutMs)) {
		cleanupIfStillOwner(layout, snapshot);
		return { action: "stopped" };
	}
	await forceKillDaemon(layout.dataDir, snapshot ?? undefined);
	return { action: "force_killed" };
}

export {
	assertDataDirPreflight,
	isForbiddenMountFstype,
	isNetworkFilesystemType,
	isWslWindowsSharePath,
	readProcessIdentity,
} from "./storage-platform.js";
