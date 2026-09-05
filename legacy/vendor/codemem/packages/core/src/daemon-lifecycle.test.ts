import { spawn } from "node:child_process";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readlinkSync,
	rmSync,
	statSync,
	symlinkSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DaemonJobService } from "./daemon-jobs.js";
import { probeDaemonWriterAvailable } from "./daemon-lifecycle.js";
import * as core from "./index.js";
import { RawEventSweeper } from "./raw-event-sweeper.js";
import {
	acquireCapabilityLifecycleLock,
	activateCapabilityManifest,
	writeCapabilityManifestGeneration,
} from "./storage.js";
import { ReadOnlyActor } from "./writer-actor.js";

const createdDirs: string[] = [];
const running: Array<{ stop: () => Promise<void> | void }> = [];

function tempDir(prefix: string): string {
	const dir = mkdtempSync(join(tmpdir(), prefix));
	createdDirs.push(dir);
	return dir;
}

function spawnSleep(seconds = 30): { pid: number; kill: () => void } {
	const child = spawn("sleep", [String(seconds)], { stdio: "ignore" });
	if (child.pid === undefined) throw new Error("failed to spawn sleep");
	return {
		pid: child.pid,
		kill: () => {
			try {
				child.kill("SIGKILL");
			} catch {
				// already gone
			}
		},
	};
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

function localManifest(modelId = "t008-local-model") {
	return core.compileDefaultCapabilityManifest({
		version: 1,
		role: "summary",
		state: "enabled",
		wireProtocol: "openai_chat_completions_v1",
		modelId,
		modelRevision: "1",
		endpointUrl: "http://127.0.0.1:1234/v1/chat/completions",
		credentialRef: { kind: "none" },
	});
}

function handshake(overrides: Partial<core.RpcRequest> = {}): core.RpcRequest {
	return {
		id: "t040-lifecycle",
		method: "GET /v1/health",
		adapter_version: "1",
		native_cli_version: "1",
		normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
		local_api_version: core.LOCAL_API_VERSION,
		capability_hash: core.RPC_CAPABILITY_HASH,
		...overrides,
	};
}

function publishActivationReceipt(
	layout: ReturnType<typeof core.resolveStorageLayout>,
	manifest: ReturnType<typeof localManifest>,
	version: 1 | 2,
	receiptId = "d5138218-b99a-48f5-9a10-ae632d5d3afb",
): void {
	const ids = [
		"cli-runtime",
		"claude-mcp",
		"claude-hooks",
		"claude-hook-runtime",
		"codex-mcp",
		"codex-hooks",
		"codex-hook-runtime",
	];
	const targets = ids.map((id) => {
		const path = join(layout.dataDir, `${id}.txt`);
		writeFileSync(path, `${id}\n`, { mode: 0o600 });
		return { id, path, fingerprint: core.sha256File(path) };
	});
	writeFileSync(
		layout.installManifestPath,
		`${JSON.stringify({ version: 1, blocks: [], targets })}\n`,
		{
			mode: 0o600,
		},
	);
	writeFileSync(
		layout.capabilityActivationReceiptPath,
		`${JSON.stringify({
			version,
			...(version === 2
				? {
						receiptId,
						activationSequence: 1,
					}
				: {}),
			configurationFingerprint: manifest.configurationFingerprint,
			targets,
		})}\n`,
		{ mode: 0o600 },
	);
}

function resumeProducerCount(layout: ReturnType<typeof core.resolveStorageLayout>): number {
	const pointer = core.readCurrentDatabasePointer(layout);
	if (!pointer) throw new Error("expected canonical database pointer");
	const db = ReadOnlyActor.open(join(layout.dbDir, pointer));
	try {
		return Number(
			(
				db.prepare("SELECT COUNT(*) AS count FROM processing_resume_producer_receipts").get() as {
					count: number;
				}
			).count,
		);
	} finally {
		db.close();
	}
}

afterEach(async () => {
	for (const handle of running.splice(0)) {
		try {
			await handle.stop();
		} catch {
			// best-effort cleanup
		}
	}
	for (const dir of createdDirs.splice(0)) {
		rmSync(dir, { recursive: true, force: true });
	}
});

describe("Phase 1 daemon lifecycle", () => {
	it("P1-T034-01-single-instance-lock", async () => {
		const dataDir = join(tempDir("codemem-daemon-lock-"), "data");
		const first = await core.startDaemon({ dataDir });
		running.push(first);

		expect(first.identity.pid).toBe(process.pid);
		expect(statSync(first.layout.controlDir).mode & 0o777).toBe(0o700);
		expect(statSync(first.lockPath).mode & 0o777).toBe(0o600);
		expect(statSync(first.socketPath).mode & 0o777).toBe(0o600);
		expect(statSync(first.identityPath).mode & 0o777).toBe(0o600);
		expect(core.readDaemonHealth(dataDir).status).toBe("ok");

		await expect(core.startDaemon({ dataDir })).rejects.toThrow(
			/already running|SQLITE_BUSY|exclusive/i,
		);

		await first.stop();
		running.pop();
		expect(core.readDaemonHealth(dataDir).status).toBe("not_running");

		const second = await core.startDaemon({ dataDir });
		running.push(second);
		expect(core.readDaemonHealth(dataDir).status).toBe("ok");
		await first.stop();
		expect(core.readDaemonHealth(dataDir).status).toBe("ok");
		expect(statSync(second.socketPath).isSocket()).toBe(true);
		await second.stop();
		running.pop();
	});

	it("rejects a competing start before mutating live restore or control state", async () => {
		const dataDir = join(tempDir("codemem-daemon-competing-start-"), "data");
		const first = await core.startDaemon({ dataDir });
		running.push(first);
		const oldPointer = core.readCurrentDatabasePointer(first.layout);
		expect(oldPointer).not.toBeNull();
		const newPointer = "versions/pending-restore.sqlite";
		core.writeStorageJournal(first.layout, {
			version: 1,
			operationId: "competing-start-restore",
			state: "switched",
			oldPointer,
			newPointer,
			artifactSha256: "0".repeat(64),
		});
		unlinkSync(first.layout.currentPointerPath);
		symlinkSync(newPointer, first.layout.currentPointerPath);
		const journalBefore = readFileSync(first.layout.journalPath, "utf8");
		const identityBefore = readFileSync(first.identityPath, "utf8");

		await expect(core.startDaemon({ dataDir })).rejects.toThrow(
			/already running|SQLITE_BUSY|exclusive/i,
		);

		expect(readlinkSync(first.layout.currentPointerPath)).toBe(newPointer);
		expect(readFileSync(first.layout.journalPath, "utf8")).toBe(journalBefore);
		expect(readFileSync(first.identityPath, "utf8")).toBe(identityBefore);
		expect(statSync(first.socketPath).isSocket()).toBe(true);
		expect(core.readDaemonHealth(dataDir).status).toBe("ok");
	});

	it("releases startup state when background initialization fails", async () => {
		const dataDir = join(tempDir("codemem-daemon-background-start-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		const failure = new Error("injected internal backfill startup failure");
		const startInternalBackfills = vi
			.spyOn(DaemonJobService.prototype, "startInternalBackfills")
			.mockImplementationOnce(() => {
				throw failure;
			});

		try {
			await expect(core.startDaemon({ dataDir })).rejects.toBe(failure);
			startInternalBackfills.mockRestore();

			expect(core.readDaemonHealth(dataDir).status).toBe("not_running");
			expect(existsSync(layout.identityPath)).toBe(false);
			expect(existsSync(layout.socketPath)).toBe(false);
			expect(probeDaemonWriterAvailable(dataDir)).toBe(true);

			const restarted = await core.startDaemon({ dataDir });
			expect(core.readDaemonHealth(dataDir).status).toBe("ok");
			await restarted.stop();
		} finally {
			startInternalBackfills.mockRestore();
			await core.stopDaemon(dataDir);
		}
	});

	it("does not start internal backfills when socket binding fails before identity publication", async () => {
		const dataDir = join(tempDir("codemem-daemon-bind-failure-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		core.ensureStorageLayout(layout);
		mkdirSync(layout.socketPath);
		const startInternalBackfills = vi.spyOn(DaemonJobService.prototype, "startInternalBackfills");

		try {
			await expect(core.startDaemon({ dataDir })).rejects.toThrow(/EISDIR|directory/i);

			expect(startInternalBackfills).not.toHaveBeenCalled();
			expect(core.readDaemonHealth(dataDir).status).toBe("not_running");
			expect(existsSync(layout.identityPath)).toBe(false);
			expect(probeDaemonWriterAvailable(dataDir)).toBe(true);
		} finally {
			startInternalBackfills.mockRestore();
			await core.stopDaemon(dataDir);
		}
	});

	it("P1-T034-02-force-kill-identity", async () => {
		const dataDir = join(tempDir("codemem-daemon-kill-"), "data");
		const victim = spawnSleep();
		try {
			const layout = core.resolveStorageLayout(dataDir);
			core.ensureStorageLayout(layout);
			const live = core.readProcessIdentity(victim.pid);
			const identity = {
				version: 1 as const,
				pid: victim.pid,
				startTime: live.startTime,
				fingerprint: live.fingerprint,
				nonce: "correct-nonce",
			};
			writeFileSync(layout.identityPath, `${JSON.stringify(identity)}\n`, {
				encoding: "utf8",
				mode: 0o600,
			});
			chmodSync(layout.identityPath, 0o600);

			const mismatched = {
				...identity,
				nonce: "wrong-nonce",
				startTime: "0",
				fingerprint: "deadbeef",
			};
			writeFileSync(layout.identityPath, `${JSON.stringify(mismatched)}\n`, {
				encoding: "utf8",
				mode: 0o600,
			});
			await expect(core.forceKillDaemon(dataDir)).rejects.toThrow(/identity|mismatch|refuse/i);
			expect(processAlive(victim.pid)).toBe(true);

			writeFileSync(layout.identityPath, `${JSON.stringify(identity)}\n`, {
				encoding: "utf8",
				mode: 0o600,
			});
			const successor = spawnSleep();
			try {
				const successorLive = core.readProcessIdentity(successor.pid);
				writeFileSync(
					layout.identityPath,
					`${JSON.stringify({
						version: 1,
						pid: successor.pid,
						startTime: successorLive.startTime,
						fingerprint: successorLive.fingerprint,
						nonce: "successor-nonce",
					})}\n`,
					{ encoding: "utf8", mode: 0o600 },
				);
				await expect(core.forceKillDaemon(dataDir, identity)).rejects.toThrow(
					/identity|mismatch|refuse/i,
				);
				expect(processAlive(victim.pid)).toBe(true);
				expect(processAlive(successor.pid)).toBe(true);
			} finally {
				successor.kill();
			}

			writeFileSync(layout.identityPath, `${JSON.stringify(identity)}\n`, {
				encoding: "utf8",
				mode: 0o600,
			});
			await core.forceKillDaemon(dataDir, identity);
			expect(processAlive(victim.pid)).toBe(false);
		} finally {
			victim.kill();
		}
	});

	it("P1-T034-03-shutdown-fallback", async () => {
		const dataDir = join(tempDir("codemem-daemon-stop-"), "data");
		const victim = spawnSleep();
		try {
			const layout = core.resolveStorageLayout(dataDir);
			core.ensureStorageLayout(layout);
			const live = core.readProcessIdentity(victim.pid);
			writeFileSync(
				layout.identityPath,
				`${JSON.stringify({
					version: 1,
					pid: victim.pid,
					startTime: live.startTime,
					fingerprint: live.fingerprint,
					nonce: "stop-nonce",
				})}\n`,
				{ encoding: "utf8", mode: 0o600 },
			);
			chmodSync(layout.identityPath, 0o600);

			const started = Date.now();
			const result = await core.stopDaemon(dataDir, { timeoutMs: 80 });
			const elapsed = Date.now() - started;
			expect(result.action).toBe("force_killed");
			expect(elapsed).toBeLessThan(1500);
			expect(processAlive(victim.pid)).toBe(false);
		} finally {
			victim.kill();
		}
	});

	it("P1-T034-04-data-dir-preflight", async () => {
		expect(core.isNetworkFilesystemType(0x6969)).toBe(true);
		expect(core.isNetworkFilesystemType(0xff534d42)).toBe(true);
		expect(core.isNetworkFilesystemType(0x65735546)).toBe(true);
		expect(core.isForbiddenMountFstype("fuse.sshfs")).toBe(true);
		expect(core.isForbiddenMountFstype("virtiofs")).toBe(true);
		expect(core.isForbiddenMountFstype("ext4")).toBe(false);
		expect(core.isWslWindowsSharePath("/mnt/c/Users/foo/.codemem")).toBe(true);
		expect(core.isWslWindowsSharePath("/mnt/d/data")).toBe(true);
		expect(core.isWslWindowsSharePath("/home/jura/.codemem")).toBe(false);

		expect(() => core.assertDataDirPreflight("/mnt/c/Users/foo/.codemem")).toThrow(
			/network|wsl|windows|share|preflight/i,
		);
		const local = join(tempDir("codemem-daemon-preflight-"), "data");
		expect(() => core.assertDataDirPreflight(local)).not.toThrow();
		expect(existsSync(local)).toBe(false);

		const linked = tempDir("codemem-daemon-symlink-");
		const dataDir = join(linked, "data");
		mkdirSync(dataDir, { mode: 0o700 });
		symlinkSync(join(linked, "elsewhere"), join(dataDir, "control"));
		await expect(core.startDaemon({ dataDir })).rejects.toThrow(/symbolic link|preflight/i);
	});
});

describe("T007 setup recovery boundary", () => {
	it("refuses an unrecoverable setup journal without deleting it or exposing prestate", async () => {
		const dataDir = join(tempDir("codemem-daemon-setup-journal-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		core.ensureStorageLayout(layout);
		const capabilitiesDir = join(layout.controlDir, "capabilities");
		const journalPath = join(capabilitiesDir, "setup-transaction.json");
		const currentPath = join(capabilitiesDir, "current");
		const secretPrestate = "t007-secret-prestate";
		mkdirSync(capabilitiesDir, { recursive: true, mode: 0o700 });
		writeFileSync(
			currentPath,
			"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
			{
				mode: 0o600,
			},
		);
		writeFileSync(journalPath, `{"version":1,"prestate":"${secretPrestate}"`, { mode: 0o600 });

		let handle: Awaited<ReturnType<typeof core.startDaemon>> | undefined;
		let startError: unknown;
		try {
			handle = await core.startDaemon({ dataDir });
			running.push(handle);
		} catch (error) {
			startError = error;
		}

		expect(startError).toBeInstanceOf(Error);
		expect(String(startError)).toMatch(/setup.*journal|journal.*recovery|recovery.*conflict/i);
		expect(String(startError)).not.toContain(secretPrestate);
		expect(readFileSync(journalPath, "utf8")).toContain(secretPrestate);
		expect(readFileSync(currentPath, "utf8")).toBe(
			"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
		);
		expect(existsSync(layout.identityPath)).toBe(false);
		expect(existsSync(layout.socketPath)).toBe(false);
	});
});

describe("T008 capability startup boundary", () => {
	it("logs a safe message and continues capture-only when the activation receipt is rejected", async () => {
		const dataDir = join(tempDir("codemem-daemon-rejected-receipt-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		const manifest = localManifest("t021-rejected-receipt");
		core.ensureStorageLayout(layout);
		writeCapabilityManifestGeneration(layout, manifest);
		const lifecycle = acquireCapabilityLifecycleLock(layout, 0);
		try {
			activateCapabilityManifest(layout, manifest.configurationFingerprint, lifecycle);
		} finally {
			lifecycle.close();
		}
		writeFileSync(layout.capabilityActivationReceiptPath, "{malformed", { mode: 0o600 });
		const log = vi.spyOn(console, "error").mockImplementation(() => undefined);
		try {
			const handle = await core.startDaemon({ dataDir });
			running.push(handle);
			expect(handle.capability).toMatchObject({
				activationReceipt: "rejected",
				providerEnabled: false,
				sweeperEnabled: false,
			});
			expect(log).toHaveBeenCalledWith("[codemem] capability activation receipt was rejected.");
		} finally {
			log.mockRestore();
		}
	});

	it("accepts legacy v1 without making it a v21 resume producer, then replays one v2 zero-match import", async () => {
		const dataDir = join(tempDir("codemem-daemon-v2-receipt-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		const manifest = localManifest("t021-v2-receipt");
		core.ensureStorageLayout(layout);
		writeCapabilityManifestGeneration(layout, manifest);
		const lifecycle = acquireCapabilityLifecycleLock(layout, 0);
		try {
			activateCapabilityManifest(layout, manifest.configurationFingerprint, lifecycle);
		} finally {
			lifecycle.close();
		}

		publishActivationReceipt(layout, manifest, 1);
		const legacy = await core.startDaemon({ dataDir });
		running.push(legacy);
		expect(legacy.capability).toMatchObject({ activationReceipt: "validated" });
		await legacy.stop();
		running.pop();
		expect(resumeProducerCount(layout)).toBe(0);

		publishActivationReceipt(layout, manifest, 2, "D5138218-B99A-48F5-9A10-AE632D5D3AFB");
		const first = await core.startDaemon({ dataDir });
		running.push(first);
		expect(first.capability).toMatchObject({
			activationReceipt: "validated",
			schemaReadiness: "ready",
			runtimeReason: "ready",
			packReadiness: "ready",
			providerEnabled: true,
			sweeperEnabled: true,
		});
		await first.stop();
		running.pop();
		expect(resumeProducerCount(layout)).toBe(1);

		publishActivationReceipt(layout, manifest, 2);
		const replay = await core.startDaemon({ dataDir });
		running.push(replay);
		expect(replay.capability).toMatchObject({
			activationReceipt: "validated",
			schemaReadiness: "ready",
			runtimeReason: "ready",
			providerEnabled: true,
			sweeperEnabled: true,
		});
		await replay.stop();
		running.pop();
		expect(resumeProducerCount(layout)).toBe(1);
	});

	it("activates the frozen Observer, AI maintenance gate, and sweeper after validation", async () => {
		const dataDir = join(tempDir("codemem-daemon-privacy-ready-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		const manifest = localManifest("t040-ready-model");
		core.ensureStorageLayout(layout);
		writeCapabilityManifestGeneration(layout, manifest);
		const lifecycle = acquireCapabilityLifecycleLock(layout, 0);
		try {
			activateCapabilityManifest(layout, manifest.configurationFingerprint, lifecycle);
		} finally {
			lifecycle.close();
		}
		publishActivationReceipt(layout, manifest, 2);

		const handle = await core.startDaemon({ dataDir });
		running.push(handle);
		expect(handle.capability).toMatchObject({
			configurationFingerprint: manifest.configurationFingerprint,
			runtimeReason: "ready",
			providerHealth: "available",
			providerEnabled: true,
			sweeperEnabled: true,
		});
		const observer = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "t040-observer-status",
				method: "GET /v1/view",
				body: { collection: "observer-status" },
			}),
		);
		expect(observer).toMatchObject({
			result: {
				status: 200,
				body: { active: { provider: "openai", model: "t040-ready-model" } },
			},
		});
		const structured = await core.callDaemonRpc(
			handle.socketPath,
			handshake({
				id: "t040-structured-maintenance",
				method: "POST /v1/jobs",
				body: { kind: "structured.backfill", args: { limit: 1 } },
			}),
		);
		expect(structured).toMatchObject({ result: { state: "queued" } });
	});

	it("starts and stops exactly one manifest sweeper across an explicit restart", async () => {
		const dataDir = join(tempDir("codemem-daemon-sweeper-restart-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		const manifest = localManifest("t040-restart-model");
		core.ensureStorageLayout(layout);
		writeCapabilityManifestGeneration(layout, manifest);
		const lifecycle = acquireCapabilityLifecycleLock(layout, 0);
		try {
			activateCapabilityManifest(layout, manifest.configurationFingerprint, lifecycle);
		} finally {
			lifecycle.close();
		}
		publishActivationReceipt(layout, manifest, 2);
		const start = vi.spyOn(RawEventSweeper.prototype, "start");
		const stop = vi.spyOn(RawEventSweeper.prototype, "stop");

		try {
			const first = await core.startDaemon({ dataDir });
			running.push(first);
			expect(start).toHaveBeenCalledTimes(1);
			await first.stop();
			running.pop();
			expect(stop).toHaveBeenCalledTimes(1);

			const restarted = await core.startDaemon({ dataDir });
			running.push(restarted);
			expect(start).toHaveBeenCalledTimes(2);
			await restarted.stop();
			running.pop();
			expect(stop).toHaveBeenCalledTimes(2);
		} finally {
			start.mockRestore();
			stop.mockRestore();
		}
	});

	it("releases the spool lock before daemon TLS preflight finishes", async () => {
		const dataDir = join(tempDir("codemem-daemon-tls-lock-"), "data");
		let acceptConnection: (socket: Socket) => void = () => {};
		const accepted = new Promise<Socket>((resolve) => {
			acceptConnection = resolve;
		});
		const server = createServer((socket) => acceptConnection(socket));
		await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
		const address = server.address();
		if (!address || typeof address === "string") throw new Error("expected TCP test address");
		const manifest = core.compileDefaultCapabilityManifest({
			version: 1,
			role: "summary",
			state: "enabled",
			wireProtocol: "openai_chat_completions_v1",
			modelId: "t008-tls-lock-model",
			modelRevision: "1",
			endpointUrl: `https://127.0.0.1:${address.port}/v1/chat/completions`,
			credentialRef: { kind: "none" },
		});
		const layout = core.resolveStorageLayout(dataDir);
		core.ensureStorageLayout(layout);
		writeCapabilityManifestGeneration(layout, manifest);
		const lifecycle = acquireCapabilityLifecycleLock(layout, 0);
		try {
			activateCapabilityManifest(layout, manifest.configurationFingerprint, lifecycle);
		} finally {
			lifecycle.close();
		}

		const pending = core.startDaemon({ dataDir });
		let socket: Socket | undefined;
		let handle: Awaited<typeof pending> | undefined;
		try {
			socket = await accepted;
			const spoolLock = core.acquireSpoolLock(dataDir, 1);
			expect(spoolLock).toBeDefined();
			spoolLock.close();
			socket.destroy();
			handle = await pending;
			running.push(handle);
		} finally {
			socket?.destroy();
			if (!handle) {
				try {
					const lateHandle = await pending;
					await lateHandle.stop();
				} catch {
					// The assertion should report the original startup failure.
				}
			}
			await new Promise<void>((resolve) => server.close(() => resolve()));
		}
	});

	it.each([
		{
			name: "malformed current pointer",
			prepare: (layout: ReturnType<typeof core.resolveStorageLayout>) => {
				writeFileSync(layout.capabilityCurrentPointerPath, "not-a-fingerprint\n", { mode: 0o600 });
			},
			reason: /pointer|fingerprint|malformed/i,
		},
		{
			name: "missing referenced generation",
			prepare: (layout: ReturnType<typeof core.resolveStorageLayout>) => {
				writeFileSync(
					layout.capabilityCurrentPointerPath,
					"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
					{ mode: 0o600 },
				);
			},
			reason: /generation.*missing|missing.*generation/i,
		},
		{
			name: "generation fingerprint mismatch",
			prepare: (layout: ReturnType<typeof core.resolveStorageLayout>) => {
				const manifest = localManifest();
				const mismatched =
					"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
				writeFileSync(
					join(layout.capabilityManifestsDir, `${mismatched}.json`),
					`${JSON.stringify(manifest)}\n`,
					{
						mode: 0o600,
					},
				);
				writeFileSync(layout.capabilityCurrentPointerPath, `${mismatched}\n`, { mode: 0o600 });
			},
			reason: /generation.*fingerprint|fingerprint.*generation|mismatch/i,
		},
	])("rejects $name before publishing daemon control state", async ({ prepare, reason }) => {
		const dataDir = join(tempDir("codemem-daemon-capability-invalid-"), "data");
		const layout = core.resolveStorageLayout(dataDir);
		core.ensureStorageLayout(layout);
		prepare(layout);
		let handle: Awaited<ReturnType<typeof core.startDaemon>> | undefined;
		let startError: unknown;
		try {
			handle = await core.startDaemon({ dataDir });
			running.push(handle);
		} catch (error) {
			startError = error;
		}

		expect(startError).toBeInstanceOf(Error);
		expect(String(startError)).toMatch(reason);
		expect(core.readDaemonHealth(dataDir).status).toBe("not_running");
		expect(existsSync(layout.identityPath)).toBe(false);
		expect(existsSync(layout.socketPath)).toBe(false);
	});
});
