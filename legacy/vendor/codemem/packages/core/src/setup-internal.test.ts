import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
	compileDefaultCapabilityManifest,
	type EffectiveCapabilityManifestV1,
} from "./capability-manifest.js";
import { probeDaemonWriterAvailable } from "./daemon-lifecycle.js";
import {
	withCapabilityLaneSetupTransaction,
	withCapabilitySetupTransaction,
} from "./setup-internal.js";
import { acquireSpoolLock } from "./spool.js";
import {
	acquireCapabilityLifecycleLock,
	type CapabilitySetupJournal,
	capabilitySetupFileState,
	ensureStorageLayout,
	resolveStorageLayout,
	writeCapabilitySetupJournal,
} from "./storage.js";

const createdDirs: string[] = [];
const managedTargetIds = [
	"cli-runtime",
	"claude-mcp",
	"claude-hooks",
	"claude-hook-runtime",
	"codex-mcp",
	"codex-hooks",
	"codex-hook-runtime",
] as const;

function dataDir(): string {
	const dir = join(mkdtempSync(join(tmpdir(), "codemem-setup-internal-")), "data");
	createdDirs.push(dir);
	return dir;
}

function manifest(endpointUrl = "http://127.0.0.1:1234/v1/chat/completions") {
	return compileDefaultCapabilityManifest({
		version: 1,
		role: "summary",
		state: "enabled",
		wireProtocol: "openai_chat_completions_v1",
		modelId: "setup-internal-test",
		modelRevision: "1",
		endpointUrl,
		credentialRef: { kind: "none" },
	});
}

function requiredJournal(
	root: string,
	inputManifest: EffectiveCapabilityManifestV1,
): CapabilitySetupJournal {
	const layout = resolveStorageLayout(root);
	const absent = capabilitySetupFileState(null, null);
	return {
		version: 1,
		phase: "prepared",
		configurationFingerprint: inputManifest.configurationFingerprint,
		targets: [
			join(layout.capabilityManifestsDir, `${inputManifest.configurationFingerprint}.json`),
			layout.capabilityCurrentPointerPath,
			layout.capabilityActivationReceiptPath,
			layout.installManifestPath,
		].map((path) => ({ path, before: absent, after: absent })),
	};
}

function publication(root: string, inputManifest: EffectiveCapabilityManifestV1) {
	const layout = resolveStorageLayout(root);
	ensureStorageLayout(layout);
	const managedTargets = managedTargetIds.map((id) => {
		const path = join(layout.dataDir, `${id}.txt`);
		const contents = `${id}\n`;
		writeFileSync(path, contents, { mode: 0o600 });
		return {
			id,
			path,
			fingerprint: createHash("sha256").update(contents).digest("hex"),
		};
	});
	const files = [
		{
			path: join(layout.capabilityManifestsDir, `${inputManifest.configurationFingerprint}.json`),
			contents: `${JSON.stringify(inputManifest)}\n`,
		},
		{
			path: layout.installManifestPath,
			contents: `${JSON.stringify({ version: 1, blocks: [], targets: managedTargets })}\n`,
		},
		{
			path: layout.capabilityActivationReceiptPath,
			contents: `${JSON.stringify({
				version: 1,
				configurationFingerprint: inputManifest.configurationFingerprint,
				targets: managedTargets,
			})}\n`,
		},
		{
			path: layout.capabilityCurrentPointerPath,
			contents: `${inputManifest.configurationFingerprint}\n`,
		},
	];
	const absent = capabilitySetupFileState(null, null);
	return {
		layout,
		journal: {
			version: 1,
			phase: "prepared",
			configurationFingerprint: inputManifest.configurationFingerprint,
			targets: files.map((file) => ({
				path: file.path,
				before: absent,
				after: capabilitySetupFileState(file.contents, 0o600),
			})),
		} satisfies CapabilitySetupJournal,
		publish(): void {
			for (const file of files) writeFileSync(file.path, file.contents, { mode: 0o600 });
		},
	};
}

afterEach(() => {
	for (const dir of createdDirs.splice(0))
		rmSync(join(dir, ".."), { recursive: true, force: true });
});

describe("withCapabilityLaneSetupTransaction", () => {
	it("holds lifecycle, spool, and writer ownership through a fresh callback", () => {
		const root = dataDir();
		const layout = resolveStorageLayout(root);

		expect(() =>
			withCapabilityLaneSetupTransaction({
				dataDir: root,
				run: () => {
					expect(() => {
						const lock = acquireCapabilityLifecycleLock(layout, 0);
						lock.close();
					}).toThrow(/busy/i);
					expect(() => {
						const lock = acquireSpoolLock(root, 1);
						lock.close();
					}).toThrow(/deadline/i);
					expect(probeDaemonWriterAvailable(root)).toBe(false);
					throw new Error("synthetic lane failure");
				},
			}),
		).toThrow(/synthetic lane failure/i);

		const lifecycle = acquireCapabilityLifecycleLock(layout, 0);
		lifecycle.close();
		const spool = acquireSpoolLock(root, 1);
		spool.close();
		expect(probeDaemonWriterAvailable(root)).toBe(true);
	});

	it("recovers an interrupted setup before entering the lane callback", () => {
		const root = dataDir();
		const layout = resolveStorageLayout(root);
		ensureStorageLayout(layout);
		const targetPath = join(root, "interrupted-lane.txt");
		writeFileSync(targetPath, "partial\n", { mode: 0o600 });
		writeCapabilitySetupJournal(layout, {
			version: 1,
			phase: "prepared",
			configurationFingerprint: manifest().configurationFingerprint,
			targets: [
				{
					path: targetPath,
					before: capabilitySetupFileState("original\n", 0o600),
					after: capabilitySetupFileState("partial\n", 0o600),
				},
			],
		});
		let called = false;

		expect(() =>
			withCapabilityLaneSetupTransaction({
				dataDir: root,
				run: () => {
					called = true;
					return true;
				},
			}),
		).toThrow(/recovered an interrupted setup/i);

		expect(called).toBe(false);
		expect(readFileSync(targetPath, "utf8")).toBe("original\n");
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(false);
	});
});

describe("withCapabilitySetupTransaction", () => {
	it("does not hold the spool lock during TLS preflight", async () => {
		const root = dataDir();
		let acceptConnection: (socket: Socket) => void = () => {};
		const accepted = new Promise<Socket>((resolve) => {
			acceptConnection = resolve;
		});
		const server = createServer((socket) => acceptConnection(socket));
		await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
		const address = server.address();
		if (!address || typeof address === "string") throw new Error("expected TCP test address");
		const inputManifest = manifest(`https://127.0.0.1:${address.port}/v1/chat/completions`);
		const pending = withCapabilitySetupTransaction({
			dataDir: root,
			manifest: inputManifest,
			expectedCurrentFingerprint: null,
			run: () => {
				throw new Error("TLS preflight unexpectedly succeeded");
			},
		});
		const failure = pending.then(
			() => null,
			(error: unknown) => error,
		);
		let socket: Socket | undefined;
		let connectionTimeout: ReturnType<typeof setTimeout> | undefined;
		try {
			const connection = await Promise.race([
				accepted,
				failure,
				new Promise<"timeout">((resolve) => {
					connectionTimeout = setTimeout(() => resolve("timeout"), 1_000);
				}),
			]);
			if (connection === "timeout") {
				throw new Error("TLS preflight did not establish a test connection within 1 second");
			}
			if (connection instanceof Error) {
				throw new Error(
					`TLS preflight failed before establishing a test connection: ${connection.message}`,
				);
			}
			if (!connection) throw new Error("TLS preflight ended before establishing a test connection");
			socket = connection;
			const lock = acquireSpoolLock(root, 1);
			lock.close();
			socket.destroy();
			expect(await failure).toBeInstanceOf(Error);
		} finally {
			if (connectionTimeout) clearTimeout(connectionTimeout);
			socket?.destroy();
			await new Promise<void>((resolve) => server.close(() => resolve()));
		}
	});

	it("holds the daemon writer lease through the callback and releases it on failure", async () => {
		const root = dataDir();
		let writerAvailableDuringCallback: boolean | undefined;

		await expect(
			withCapabilitySetupTransaction({
				dataDir: root,
				manifest: manifest(),
				expectedCurrentFingerprint: null,
				run: () => {
					writerAvailableDuringCallback = probeDaemonWriterAvailable(root);
					throw new Error("synthetic setup failure");
				},
			}),
		).rejects.toThrow(/synthetic setup failure/i);

		expect(writerAvailableDuringCallback).toBe(false);
		expect(probeDaemonWriterAvailable(root)).toBe(true);
	});

	it("recovers an interrupted journal before unavailable provider TLS", async () => {
		const root = dataDir();
		const layout = resolveStorageLayout(root);
		ensureStorageLayout(layout);
		const sentinelPath = join(root, "interrupted.txt");
		const untouchedPath = join(root, "untouched.txt");
		writeFileSync(sentinelPath, "partial\n", { mode: 0o600 });

		const serverSockets = new Set<Socket>();
		let connections = 0;
		const server = createServer((socket) => {
			connections += 1;
			serverSockets.add(socket);
			socket.once("close", () => serverSockets.delete(socket));
		});
		await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
		const address = server.address();
		if (!address || typeof address === "string") throw new Error("expected TCP test address");
		const inputManifest = manifest(`https://127.0.0.1:${address.port}/v1/chat/completions`);
		writeCapabilitySetupJournal(layout, {
			version: 1,
			phase: "prepared",
			configurationFingerprint: inputManifest.configurationFingerprint,
			targets: [
				{
					path: sentinelPath,
					before: capabilitySetupFileState("original\n", 0o600),
					after: capabilitySetupFileState("partial\n", 0o600),
				},
				{
					path: untouchedPath,
					before: capabilitySetupFileState(null, null),
					after: capabilitySetupFileState("planned\n", 0o600),
				},
			],
		});

		const settled = withCapabilitySetupTransaction({
			dataDir: root,
			manifest: inputManifest,
			expectedCurrentFingerprint: null,
			run: () => {
				throw new Error("recovered setup unexpectedly continued");
			},
		}).then(
			() => null,
			(error: unknown) => error,
		);
		let timeout: ReturnType<typeof setTimeout> | undefined;
		try {
			const outcome = await Promise.race([
				settled,
				new Promise<"timeout">((resolve) => {
					timeout = setTimeout(() => resolve("timeout"), 1_000);
				}),
			]);
			expect(outcome).not.toBe("timeout");
			expect(String(outcome)).toMatch(/recovered an interrupted setup/i);
			expect(connections).toBe(0);
			expect(readFileSync(sentinelPath, "utf8")).toBe("original\n");
			expect(existsSync(untouchedPath)).toBe(false);
			expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(false);
		} finally {
			if (timeout) clearTimeout(timeout);
			for (const socket of serverSockets) socket.destroy();
			await settled;
			await new Promise<void>((resolve) => server.close(() => resolve()));
		}
	});

	it.each([
		"fingerprint",
		"targets",
	] as const)("rejects a journal with an invalid %s binding", async (invalid) => {
		const root = dataDir();
		const inputManifest = manifest();
		const journal = requiredJournal(root, inputManifest);
		if (invalid === "fingerprint") {
			journal.configurationFingerprint = `sha256:${"a".repeat(64)}`;
		} else {
			journal.targets.pop();
		}

		await expect(
			withCapabilitySetupTransaction({
				dataDir: root,
				manifest: inputManifest,
				expectedCurrentFingerprint: null,
				run: ({ writeJournal }) => writeJournal(journal),
			}),
		).rejects.toThrow(/manifest|required publication target/i);
	});

	it("requires both a journal and completed finalization", async () => {
		const withoutJournal = dataDir();
		const inputManifest = manifest();
		await expect(
			withCapabilitySetupTransaction({
				dataDir: withoutJournal,
				manifest: inputManifest,
				expectedCurrentFingerprint: null,
				run: () => "not committed",
			}),
		).rejects.toThrow(/journal was not written/i);

		const withoutFinalization = dataDir();
		await expect(
			withCapabilitySetupTransaction({
				dataDir: withoutFinalization,
				manifest: inputManifest,
				expectedCurrentFingerprint: null,
				run: ({ writeJournal }) =>
					writeJournal(requiredJournal(withoutFinalization, inputManifest)),
			}),
		).rejects.toThrow(/not finalized/i);
	});

	it.each([
		["active manifest", "capabilityCurrentPointerPath", /publish the disclosed manifest/i],
		["receipt", "capabilityActivationReceiptPath", /valid activation receipt/i],
	] as const)("returns only after validating the %s", async (_name, invalidPath, error) => {
		const root = dataDir();
		const inputManifest = manifest();
		const prepared = publication(root, inputManifest);
		await expect(
			withCapabilitySetupTransaction({
				dataDir: root,
				manifest: inputManifest,
				expectedCurrentFingerprint: null,
				run: ({ writeJournal, recover }) => {
					writeJournal(prepared.journal);
					prepared.publish();
					recover();
					unlinkSync(prepared.layout[invalidPath]);
					return "invalid success";
				},
			}),
		).rejects.toThrow(error);
	});
});
