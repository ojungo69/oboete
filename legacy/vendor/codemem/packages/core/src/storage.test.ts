import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
	chmodSync,
	existsSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readlinkSync,
	rmSync,
	statSync,
	truncateSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
	compileCapabilityManifest,
	compileDefaultCapabilityManifest,
} from "./capability-manifest.js";
import * as storage from "./storage.js";

const recoveryRace = vi.hoisted(() => ({
	path: "",
	openCount: 0,
	mutateOnOpen: 0,
	swapOnReadPath: "",
	swapTarget: "",
	beforeLinkPath: "",
	beforeRenamePath: "",
	beforeRenameMutation: null as null | ((path: string) => void),
	beforeSymlinkPath: "",
	capturedDev: 0,
	capturedIno: 0,
	replaceWithDirectory: false,
	renamedFrom: [] as string[],
	text: "",
}));

vi.mock("node:fs", async (importOriginal) => {
	const actual = await importOriginal<typeof import("node:fs")>();
	const swapPath = () => {
		if (!recoveryRace.swapOnReadPath) return;
		const path = recoveryRace.swapOnReadPath;
		recoveryRace.swapOnReadPath = "";
		actual.rmSync(path, { force: true });
		actual.symlinkSync(recoveryRace.swapTarget, path);
	};
	return {
		...actual,
		openSync(...args: Parameters<typeof actual.openSync>) {
			const descriptor = actual.openSync(...args);
			if (String(args[0]) === recoveryRace.swapOnReadPath) swapPath();
			if (String(args[0]) === recoveryRace.path) {
				recoveryRace.openCount += 1;
				if (recoveryRace.openCount === recoveryRace.mutateOnOpen) {
					actual.writeFileSync(recoveryRace.path, recoveryRace.text, { mode: 0o600 });
				}
			}
			return descriptor;
		},
		readFileSync(...args: Parameters<typeof actual.readFileSync>) {
			if (String(args[0]) === recoveryRace.swapOnReadPath) swapPath();
			return actual.readFileSync(...args);
		},
		linkSync(...args: Parameters<typeof actual.linkSync>) {
			if (String(args[1]) === recoveryRace.beforeLinkPath) {
				actual.writeFileSync(recoveryRace.beforeLinkPath, recoveryRace.text, { mode: 0o600 });
				recoveryRace.beforeLinkPath = "";
			}
			return actual.linkSync(...args);
		},
		renameSync(...args: Parameters<typeof actual.renameSync>) {
			recoveryRace.renamedFrom.push(String(args[0]));
			if (String(args[0]) === recoveryRace.beforeRenamePath) {
				const path = recoveryRace.beforeRenamePath;
				const mutation = recoveryRace.beforeRenameMutation;
				recoveryRace.beforeRenamePath = "";
				recoveryRace.beforeRenameMutation = null;
				if (mutation) {
					mutation(path);
				} else if (recoveryRace.replaceWithDirectory) {
					actual.rmSync(path, { force: true });
					actual.mkdirSync(path, { mode: 0o700 });
					actual.writeFileSync(`${path}/nested-sentinel.txt`, recoveryRace.text);
				} else {
					actual.writeFileSync(path, recoveryRace.text, { mode: 0o600 });
				}
			}
			return actual.renameSync(...args);
		},
		symlinkSync(...args: Parameters<typeof actual.symlinkSync>) {
			if (String(args[1]) === recoveryRace.beforeSymlinkPath) {
				actual.writeFileSync(recoveryRace.beforeSymlinkPath, recoveryRace.text, { mode: 0o600 });
				recoveryRace.beforeSymlinkPath = "";
			}
			return actual.symlinkSync(...args);
		},
	};
});

type JsonObject = Record<string, unknown>;
type CapabilityStorageLayout = ReturnType<typeof storage.resolveStorageLayout> & {
	capabilitiesDir: string;
	capabilityManifestsDir: string;
	capabilityCurrentPointerPath: string;
	capabilityLifecycleLockPath: string;
	capabilityActivationReceiptPath: string;
	capabilitySetupTransactionPath: string;
};
type CapabilityLifecycleLease = { close(): void };
type WriteCapabilityManifestGeneration = (
	layout: CapabilityStorageLayout,
	manifest: JsonObject,
) => void;
type ReadCapabilityManifestGeneration = (
	layout: CapabilityStorageLayout,
	fingerprint: string,
) => JsonObject;
type AcquireCapabilityLifecycleLock = (
	layout: CapabilityStorageLayout,
	deadlineMs?: number,
) => CapabilityLifecycleLease;
type ActivateCapabilityManifest = (
	layout: CapabilityStorageLayout,
	fingerprint: string,
	lease: CapabilityLifecycleLease,
) => void;
type ReadCurrentCapabilityManifest = (layout: CapabilityStorageLayout) => JsonObject | null;

const fixturePath = fileURLToPath(
	new URL(
		"../../../../../specs/005-product-reset/fixtures/slice1-bidirectional-en-v1.json",
		import.meta.url,
	),
);
const fixture = JSON.parse(readFileSync(fixturePath, "utf8")) as {
	effectiveConfiguration: JsonObject;
	localDerivationManifest: JsonObject;
	outputLimitRecoveryManifest: JsonObject;
};
const baseManifest = fixture.effectiveConfiguration;
const fingerprint = baseManifest.configurationFingerprint as string;
const createdDirs: string[] = [];

function tempLayout(): CapabilityStorageLayout {
	const root = mkdtempSync(join(tmpdir(), "codemem-capability-storage-"));
	createdDirs.push(root);
	return storage.resolveStorageLayout(join(root, "data")) as CapabilityStorageLayout;
}

function storageFunction<T>(name: string): T {
	const value = Reflect.get(storage, name);
	expect(value, `${name} must be exported by storage.ts`).toBeTypeOf("function");
	return value as T;
}

function providerProposal(provider: JsonObject): JsonObject {
	const {
		providerFingerprint: _providerFingerprint,
		executionLocation: _executionLocation,
		egressPolicy: _egressPolicy,
		costClass: _costClass,
		tlsPolicy: _tlsPolicy,
		redirectPolicy: _redirectPolicy,
		...proposal
	} = provider;
	return proposal;
}

function manifestProposal(manifest: JsonObject): JsonObject {
	const {
		configurationFingerprint: _configurationFingerprint,
		summaryProvider,
		...proposal
	} = manifest;
	return { ...proposal, summaryProvider: providerProposal(summaryProvider as JsonObject) };
}

function recoveryPaths(
	configurationFingerprint: string,
	index: number,
	target: storage.CapabilitySetupTarget,
	direction: "rollback" | "compensate" = "rollback",
): { claim: string; prior: string } {
	const from = direction === "rollback" ? target.after : target.before;
	const to = direction === "rollback" ? target.before : target.after;
	const recoveryId = createHash("sha256")
		.update(
			JSON.stringify([
				configurationFingerprint,
				index,
				direction,
				target.path,
				from.sha256,
				from.mode,
				to.sha256,
				to.mode,
			]),
		)
		.digest("hex")
		.slice(0, 32);
	const recoveryDir = join(dirname(target.path), `.codemem-setup-restore-${recoveryId}`);
	mkdirSync(recoveryDir, { recursive: true, mode: 0o700 });
	chmodSync(recoveryDir, 0o700);
	return { claim: join(recoveryDir, "claim"), prior: join(recoveryDir, "prior") };
}

function delayedFifoWriter(path: string): ReturnType<typeof spawn> {
	return spawn(
		process.execPath,
		[
			"-e",
			"setTimeout(() => { const fs = require('node:fs'); const fd = fs.openSync(process.argv[1], fs.constants.O_WRONLY); fs.closeSync(fd); }, 1000)",
			path,
		],
		{ stdio: "ignore" },
	);
}

function expectFifoConflictWithoutBlocking(path: string, action: () => unknown): void {
	execFileSync("mkfifo", [path]);
	const writer = delayedFifoWriter(path);
	const startedAt = performance.now();
	try {
		expect(action).toThrow(/conflict/i);
	} finally {
		writer.kill("SIGKILL");
	}
	expect(performance.now() - startedAt).toBeLessThan(500);
}

afterEach(() => {
	recoveryRace.path = "";
	recoveryRace.openCount = 0;
	recoveryRace.mutateOnOpen = 0;
	recoveryRace.swapOnReadPath = "";
	recoveryRace.swapTarget = "";
	recoveryRace.beforeLinkPath = "";
	recoveryRace.beforeRenamePath = "";
	recoveryRace.beforeRenameMutation = null;
	recoveryRace.beforeSymlinkPath = "";
	recoveryRace.capturedDev = 0;
	recoveryRace.capturedIno = 0;
	recoveryRace.replaceWithDirectory = false;
	recoveryRace.renamedFrom.length = 0;
	recoveryRace.text = "";
	for (const dir of createdDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("Slice 1 capability storage", () => {
	it("resolves the one closed capability storage layout", () => {
		const layout = tempLayout();

		expect(layout.capabilitiesDir).toBe(join(layout.controlDir, "capabilities"));
		expect(layout.capabilityManifestsDir).toBe(join(layout.capabilitiesDir, "manifests"));
		expect(layout.capabilityCurrentPointerPath).toBe(join(layout.capabilitiesDir, "current"));
		expect(layout.capabilityLifecycleLockPath).toBe(join(layout.capabilitiesDir, "lifecycle.lock"));
		expect(layout.capabilityActivationReceiptPath).toBe(
			join(layout.capabilitiesDir, "activation-receipt.json"),
		);
		expect(layout.capabilitySetupTransactionPath).toBe(
			join(layout.capabilitiesDir, "setup-transaction.json"),
		);
	});

	it("writes an owner-only immutable generation and reads it by fingerprint", () => {
		const layout = tempLayout();
		const writeGeneration = storageFunction<WriteCapabilityManifestGeneration>(
			"writeCapabilityManifestGeneration",
		);
		const readGeneration = storageFunction<ReadCapabilityManifestGeneration>(
			"readCapabilityManifestGeneration",
		);

		writeGeneration(layout, baseManifest);
		const generationPath = join(layout.capabilityManifestsDir, `${fingerprint}.json`);
		expect(readGeneration(layout, fingerprint)).toEqual(baseManifest);
		expect(statSync(generationPath).mode & 0o777).toBe(0o600);

		const original = readGeneration(layout, fingerprint);
		expect(() =>
			writeGeneration(layout, { ...baseManifest, manifestId: "externally-mutated" }),
		).toThrow();
		expect(readGeneration(layout, fingerprint)).toEqual(original);
	});

	it("rejects an oversized current pointer without reading or mutating it", () => {
		const layout = tempLayout();
		storage.writeCapabilityManifestGeneration(layout, baseManifest);
		writeFileSync(layout.capabilityCurrentPointerPath, `${fingerprint}\n`, { mode: 0o600 });
		truncateSync(layout.capabilityCurrentPointerPath, 129);

		expect(() => storage.readCurrentCapabilityManifest(layout)).toThrow(/not a regular file/i);
		expect(lstatSync(layout.capabilityCurrentPointerPath).size).toBe(129);
		expect(existsSync(join(layout.capabilityManifestsDir, `${fingerprint}.json`))).toBe(true);
	});

	it("rejects an oversized capability generation without reading or replacing it", () => {
		const layout = tempLayout();
		storage.writeCapabilityManifestGeneration(layout, baseManifest);
		const generationPath = join(layout.capabilityManifestsDir, `${fingerprint}.json`);
		truncateSync(generationPath, 1024 * 1024 + 1);

		expect(() => storage.writeCapabilityManifestGeneration(layout, baseManifest)).toThrow(
			/not a regular file/i,
		);
		expect(lstatSync(generationPath).size).toBe(1024 * 1024 + 1);
		expect(existsSync(layout.capabilityCurrentPointerPath)).toBe(false);
	});

	it("rejects a current pointer swapped to a symlink between inspection and read", () => {
		const layout = tempLayout();
		storage.writeCapabilityManifestGeneration(layout, baseManifest);
		const externalPointerPath = join(layout.dataDir, "external-current-pointer.txt");
		writeFileSync(externalPointerPath, `${fingerprint}\n`, { mode: 0o600 });
		writeFileSync(layout.capabilityCurrentPointerPath, `${fingerprint}\n`, { mode: 0o600 });
		recoveryRace.swapOnReadPath = layout.capabilityCurrentPointerPath;
		recoveryRace.swapTarget = externalPointerPath;

		expect(() => storage.readCurrentCapabilityManifest(layout)).toThrow(/not a regular file/i);
		expect(lstatSync(layout.capabilityCurrentPointerPath).isSymbolicLink()).toBe(true);
		expect(readlinkSync(layout.capabilityCurrentPointerPath)).toBe(externalPointerPath);
		expect(readFileSync(externalPointerPath, "utf8")).toBe(`${fingerprint}\n`);
	});

	it("requires the lifecycle lease before publishing current and preserves the old pointer on failure", () => {
		const layout = tempLayout();
		const writeGeneration = storageFunction<WriteCapabilityManifestGeneration>(
			"writeCapabilityManifestGeneration",
		);
		const acquireLifecycleLock = storageFunction<AcquireCapabilityLifecycleLock>(
			"acquireCapabilityLifecycleLock",
		);
		const activateManifest = storageFunction<ActivateCapabilityManifest>(
			"activateCapabilityManifest",
		);
		const readCurrentManifest = storageFunction<ReadCurrentCapabilityManifest>(
			"readCurrentCapabilityManifest",
		);
		writeGeneration(layout, baseManifest);

		expect(() => activateManifest(layout, fingerprint, undefined as never)).toThrow();
		expect(existsSync(layout.capabilityCurrentPointerPath)).toBe(false);

		const lease = acquireLifecycleLock(layout, 0);
		try {
			activateManifest(layout, fingerprint, lease);
			expect(readCurrentManifest(layout)).toEqual(baseManifest);
			expect(readFileSync(layout.capabilityCurrentPointerPath, "utf8").trim()).toBe(fingerprint);

			const missingFingerprint = `sha256:${"f".repeat(64)}`;
			expect(() => activateManifest(layout, missingFingerprint, lease)).toThrow();
			expect(readCurrentManifest(layout)).toEqual(baseManifest);
			expect(readFileSync(layout.capabilityCurrentPointerPath, "utf8").trim()).toBe(fingerprint);
		} finally {
			lease.close();
		}
	});

	it("uses one exclusive lifecycle lock for setup and daemon-start ordering", () => {
		const layout = tempLayout();
		const acquireLifecycleLock = storageFunction<AcquireCapabilityLifecycleLock>(
			"acquireCapabilityLifecycleLock",
		);
		const first = acquireLifecycleLock(layout, 0);

		try {
			expect(() => acquireLifecycleLock(layout, 0)).toThrow();
		} finally {
			first.close();
		}

		const afterRelease = acquireLifecycleLock(layout, 0);
		afterRelease.close();
	});

	it("refuses recovery from a setup journal that is not owner-only", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = `${layout.dataDir}/../outside-target.txt`;
		const untouchedPath = `${layout.dataDir}/../not-yet-written.txt`;
		writeFileSync(targetPath, "journal-owned\n", { mode: 0o600 });
		const journal = {
			version: 1 as const,
			phase: "prepared" as const,
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: untouchedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("planned\n", 0o600),
				},
			],
		};
		writeFileSync(layout.capabilitySetupTransactionPath, `${JSON.stringify(journal)}\n`);
		chmodSync(layout.capabilitySetupTransactionPath, 0o644);
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/owner-only/i);
		} finally {
			lease.close();
		}
		expect(readFileSync(targetPath, "utf8")).toBe("journal-owned\n");
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("rejects setup journal targets with equivalent absolute paths", () => {
		const layout = tempLayout();
		const targetPath = join(layout.dataDir, "duplicate-target.txt");
		const aliasPath = `${layout.dataDir}/./duplicate-target.txt`;
		const before = storage.capabilitySetupFileState("before\n", 0o600);
		const after = storage.capabilitySetupFileState("after\n", 0o600);

		expect(() =>
			storage.writeCapabilitySetupJournal(layout, {
				version: 1,
				phase: "prepared",
				configurationFingerprint: fingerprint,
				targets: [
					{ path: targetPath, before, after },
					{ path: aliasPath, before, after },
				],
			}),
		).toThrow(/target path/i);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(false);
	});

	it("rejects an oversized setup target before reading or mutating it", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "oversized-target.txt");
		const untouchedPath = join(layout.dataDir, "oversized-target-untouched.txt");
		writeFileSync(targetPath, "journal-owned\n", { mode: 0o600 });
		storage.writeCapabilitySetupJournal(layout, {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: untouchedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("planned\n", 0o600),
				},
			],
		});
		truncateSync(targetPath, 4_500_001);
		const startedAt = performance.now();
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(performance.now() - startedAt).toBeLessThan(500);
		expect(lstatSync(targetPath).size).toBe(4_500_001);
		expect(existsSync(untouchedPath)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("rejects an oversized setup journal before parsing or mutating targets", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "oversized-journal-target.txt");
		writeFileSync(targetPath, "unchanged\n", { mode: 0o600 });
		writeFileSync(layout.capabilitySetupTransactionPath, "{}\n", { mode: 0o600 });
		truncateSync(layout.capabilitySetupTransactionPath, 32 * 1024 * 1024 + 1);
		const startedAt = performance.now();
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/journal/i);
		} finally {
			lease.close();
		}
		expect(performance.now() - startedAt).toBeLessThan(500);
		expect(readFileSync(targetPath, "utf8")).toBe("unchanged\n");
		expect(lstatSync(layout.capabilitySetupTransactionPath).size).toBe(32 * 1024 * 1024 + 1);
	});

	it("rejects an on-disk journal whose combined target states exceed the total budget", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "state-budget-target.txt");
		writeFileSync(targetPath, "unchanged\n", { mode: 0o600 });
		const contentsBase64 = "A".repeat(4_000_004);
		const state = {
			contentsBase64,
			mode: 0o600,
			sha256: createHash("sha256").update(Buffer.from(contentsBase64, "base64")).digest("hex"),
		};
		let journalContents = `${JSON.stringify({
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [0, 1, 2].map((index) => ({
				path: index === 0 ? targetPath : join(layout.dataDir, `state-budget-${index}.txt`),
				before: state,
				after: state,
			})),
		})}\n`;
		const journalBytes = Buffer.byteLength(journalContents);
		expect(journalBytes).toBeLessThan(32 * 1024 * 1024);
		writeFileSync(layout.capabilitySetupTransactionPath, journalContents, { mode: 0o600 });
		journalContents = "";
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(
				/journal recovery failed/i,
			);
		} finally {
			lease.close();
		}
		expect(readFileSync(targetPath, "utf8")).toBe("unchanged\n");
		expect(existsSync(join(layout.dataDir, "state-budget-1.txt"))).toBe(false);
		expect(existsSync(join(layout.dataDir, "state-budget-2.txt"))).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
		expect(lstatSync(layout.capabilitySetupTransactionPath).size).toBe(journalBytes);
	});

	it("rechecks every setup target before rollback and preserves external drift", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "managed-target.txt");
		const untouchedPath = join(layout.dataDir, "untouched-target.txt");
		writeFileSync(targetPath, "journal-owned\n", { mode: 0o600 });
		storage.writeCapabilitySetupJournal(layout, {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: untouchedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("planned\n", 0o600),
				},
			],
		});
		recoveryRace.path = targetPath;
		recoveryRace.mutateOnOpen = 2;
		recoveryRace.text = "external-edit\n";
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(readFileSync(targetPath, "utf8")).toBe("external-edit\n");
		expect(existsSync(untouchedPath)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("quarantines with compare-and-publish semantics instead of overwriting a late save", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "late-save-target.txt");
		const compensatedPath = join(layout.dataDir, "compensated-target.txt");
		const untouchedPath = join(layout.dataDir, "late-save-untouched.txt");
		writeFileSync(targetPath, "journal-owned\n", { mode: 0o600 });
		writeFileSync(compensatedPath, "second-journal-owned\n", { mode: 0o600 });
		storage.writeCapabilitySetupJournal(layout, {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: compensatedPath,
					before: storage.capabilitySetupFileState("second-original\n", 0o600),
					after: storage.capabilitySetupFileState("second-journal-owned\n", 0o600),
				},
				{
					path: untouchedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("planned\n", 0o600),
				},
			],
		});
		recoveryRace.beforeRenamePath = targetPath;
		recoveryRace.text = "late-external-save\n";
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(readFileSync(targetPath, "utf8")).toBe("late-external-save\n");
		expect(readFileSync(compensatedPath, "utf8")).toBe("second-journal-owned\n");
		expect(existsSync(untouchedPath)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("keeps a directory swapped in before quarantine reachable without overwriting the target", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "directory-race-target.txt");
		const untouchedPath = join(layout.dataDir, "directory-race-untouched.txt");
		writeFileSync(targetPath, "journal-owned\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: untouchedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("planned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		recoveryRace.beforeRenamePath = targetPath;
		recoveryRace.replaceWithDirectory = true;
		recoveryRace.text = "nested-sentinel\n";
		const { claim } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
		);
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(statSync(targetPath).isDirectory()).toBe(true);
		expect(lstatSync(targetPath).isSymbolicLink()).toBe(true);
		expect(readlinkSync(targetPath)).toBe(claim);
		expect(lstatSync(claim).isDirectory()).toBe(true);
		expect(readFileSync(join(targetPath, "nested-sentinel.txt"), "utf8")).toBe("nested-sentinel\n");
		expect(existsSync(untouchedPath)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("does not overwrite a late target while preserving a non-hardlinkable capture", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "directory-late-save-target.txt");
		const untouchedPath = join(layout.dataDir, "directory-late-save-untouched.txt");
		writeFileSync(targetPath, "journal-owned\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: untouchedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("planned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
		);
		recoveryRace.beforeRenamePath = targetPath;
		recoveryRace.beforeSymlinkPath = targetPath;
		recoveryRace.replaceWithDirectory = true;
		recoveryRace.text = "late-external-save\n";
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(readFileSync(targetPath, "utf8")).toBe("late-external-save\n");
		expect(lstatSync(claim).isDirectory()).toBe(true);
		expect(readFileSync(join(claim, "nested-sentinel.txt"), "utf8")).toBe("late-external-save\n");
		expect(existsSync(untouchedPath)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("restores a FIFO swapped in immediately before quarantine without blocking", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "fifo-quarantine-race-target.txt");
		const untouchedPath = join(layout.dataDir, "fifo-quarantine-race-untouched.txt");
		writeFileSync(targetPath, "journal-owned\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: untouchedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("planned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
		);
		recoveryRace.beforeRenamePath = targetPath;
		recoveryRace.beforeRenameMutation = (path) => {
			rmSync(path, { force: true });
			execFileSync("mkfifo", [path]);
			const captured = lstatSync(path);
			recoveryRace.capturedDev = captured.dev;
			recoveryRace.capturedIno = captured.ino;
		};
		const writer = delayedFifoWriter(claim);
		const startedAt = performance.now();
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
			writer.kill("SIGKILL");
		}
		expect(performance.now() - startedAt).toBeLessThan(500);
		const restored = lstatSync(targetPath);
		expect(restored.isFIFO()).toBe(true);
		expect(restored.dev).toBe(recoveryRace.capturedDev);
		expect(restored.ino).toBe(recoveryRace.capturedIno);
		expect(existsSync(claim)).toBe(false);
		expect(existsSync(dirname(claim))).toBe(false);
		expect(existsSync(untouchedPath)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("cleans validated crash sidecars when the visible target is already restored", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "restored-before-crash.txt");
		writeFileSync(targetPath, "original\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim, prior } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
		);
		writeFileSync(claim, "journal-owned\n", { mode: 0o600 });
		writeFileSync(prior, "original\n", { mode: 0o600 });
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(storage.recoverCapabilitySetupTransaction(layout, lease)).toEqual({
				action: "rolled_back",
			});
		} finally {
			lease.close();
		}
		expect(readFileSync(targetPath, "utf8")).toBe("original\n");
		expect(existsSync(claim)).toBe(false);
		expect(existsSync(prior)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(false);
	});

	it("fails closed when both recovery directions have pending sidecars", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, "ambiguous-direction-target.txt");
		writeFileSync(targetPath, "original\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const rollback = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
			"rollback",
		);
		const compensate = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
			"compensate",
		);
		writeFileSync(rollback.claim, "journal-owned\n", { mode: 0o600 });
		writeFileSync(compensate.claim, "original\n", { mode: 0o600 });
		recoveryRace.renamedFrom.length = 0;
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(recoveryRace.renamedFrom).not.toContain(targetPath);
		expect(readFileSync(targetPath, "utf8")).toBe("original\n");
		expect(existsSync(rollback.claim)).toBe(true);
		expect(existsSync(compensate.claim)).toBe(true);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("compensates a resumed target when the later main rollback conflicts", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const resumedTargetPath = join(layout.dataDir, "resumed-before-main-rollback.txt");
		const conflictingTargetPath = join(layout.dataDir, "main-rollback-conflict.txt");
		const ordinaryTargetPath = join(layout.dataDir, "ordinary-main-rollback.txt");
		writeFileSync(conflictingTargetPath, "conflict-journal-owned\n", { mode: 0o600 });
		writeFileSync(ordinaryTargetPath, "ordinary-journal-owned\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: resumedTargetPath,
					before: storage.capabilitySetupFileState("resumed-original\n", 0o600),
					after: storage.capabilitySetupFileState("resumed-journal-owned\n", 0o600),
				},
				{
					path: conflictingTargetPath,
					before: storage.capabilitySetupFileState("conflict-original\n", 0o600),
					after: storage.capabilitySetupFileState("conflict-journal-owned\n", 0o600),
				},
				{
					path: ordinaryTargetPath,
					before: storage.capabilitySetupFileState("ordinary-original\n", 0o600),
					after: storage.capabilitySetupFileState("ordinary-journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const resumedTarget = journal.targets[0] as storage.CapabilitySetupTarget;
		const { claim, prior } = recoveryPaths(fingerprint, 0, resumedTarget, "rollback");
		writeFileSync(claim, "resumed-journal-owned\n", { mode: 0o600 });
		writeFileSync(prior, "resumed-original\n", { mode: 0o600 });
		recoveryRace.beforeRenamePath = conflictingTargetPath;
		recoveryRace.text = "conflict-external-edit\n";
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(readFileSync(resumedTargetPath, "utf8")).toBe("resumed-journal-owned\n");
		expect(readFileSync(conflictingTargetPath, "utf8")).toBe("conflict-external-edit\n");
		expect(readFileSync(ordinaryTargetPath, "utf8")).toBe("ordinary-journal-owned\n");
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it.each([
		{ name: "claim hash", sidecar: "claim" as const, contents: "unknown\n", mode: 0o600 },
		{ name: "prior mode", sidecar: "prior" as const, contents: "original\n", mode: 0o644 },
	])("fails closed on unknown $name residue beside an already-restored target", (residue) => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, `unknown-${residue.sidecar}-residue.txt`);
		writeFileSync(targetPath, "original\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim, prior } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
		);
		writeFileSync(claim, residue.sidecar === "claim" ? residue.contents : "journal-owned\n", {
			mode: residue.sidecar === "claim" ? residue.mode : 0o600,
		});
		writeFileSync(prior, residue.sidecar === "prior" ? residue.contents : "original\n", {
			mode: residue.sidecar === "prior" ? residue.mode : 0o600,
		});
		if (residue.sidecar === "prior") chmodSync(prior, residue.mode);
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(readFileSync(targetPath, "utf8")).toBe("original\n");
		expect(existsSync(claim)).toBe(true);
		expect(existsSync(prior)).toBe(true);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it.each([
		"target",
		"claim",
	] as const)("rejects a FIFO at a deterministic recovery %s without blocking", (fifoLocation) => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const targetPath = join(layout.dataDir, `fifo-${fifoLocation}-target.txt`);
		if (fifoLocation === "claim") {
			writeFileSync(targetPath, "original\n", { mode: 0o600 });
		}
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: targetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
		);
		const fifoPath = fifoLocation === "target" ? targetPath : claim;
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expectFifoConflictWithoutBlocking(fifoPath, () =>
				storage.recoverCapabilitySetupTransaction(layout, lease),
			);
		} finally {
			lease.close();
		}
		expect(lstatSync(fifoPath).isFIFO()).toBe(true);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("reconciles sidecars left by a crash after compensation", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const firstTargetPath = join(layout.dataDir, "compensation-crash-first.txt");
		const compensatedTargetPath = join(layout.dataDir, "compensation-crash-second.txt");
		writeFileSync(firstTargetPath, "first-original\n", { mode: 0o600 });
		writeFileSync(compensatedTargetPath, "second-journal-owned\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: firstTargetPath,
					before: storage.capabilitySetupFileState("first-original\n", 0o600),
					after: storage.capabilitySetupFileState("first-journal-owned\n", 0o600),
				},
				{
					path: compensatedTargetPath,
					before: storage.capabilitySetupFileState("second-original\n", 0o600),
					after: storage.capabilitySetupFileState("second-journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim, prior } = recoveryPaths(
			fingerprint,
			1,
			journal.targets[1] as storage.CapabilitySetupTarget,
			"compensate",
		);
		writeFileSync(claim, "second-original\n", { mode: 0o600 });
		writeFileSync(prior, "second-journal-owned\n", { mode: 0o600 });
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(storage.recoverCapabilitySetupTransaction(layout, lease)).toEqual({
				action: "rolled_back",
			});
		} finally {
			lease.close();
		}
		expect(readFileSync(firstTargetPath, "utf8")).toBe("first-original\n");
		expect(readFileSync(compensatedTargetPath, "utf8")).toBe("second-original\n");
		expect(existsSync(claim)).toBe(false);
		expect(existsSync(prior)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(false);
	});

	it("compensates an earlier resumption when a later resumption conflicts", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const firstTargetPath = join(layout.dataDir, "first-pending-resumption.txt");
		const secondTargetPath = join(layout.dataDir, "second-pending-resumption.txt");
		const unrelatedPath = join(layout.dataDir, "unrelated-not-published.txt");
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: firstTargetPath,
					before: storage.capabilitySetupFileState("first-original\n", 0o600),
					after: storage.capabilitySetupFileState("first-journal-owned\n", 0o600),
				},
				{
					path: secondTargetPath,
					before: storage.capabilitySetupFileState("second-original\n", 0o600),
					after: storage.capabilitySetupFileState("second-journal-owned\n", 0o600),
				},
				{
					path: unrelatedPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState("unrelated-planned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		for (const index of [0, 1]) {
			const target = journal.targets[index] as storage.CapabilitySetupTarget;
			const { claim, prior } = recoveryPaths(fingerprint, index, target, "rollback");
			writeFileSync(claim, Buffer.from(target.after.contentsBase64 as string, "base64"), {
				mode: 0o600,
			});
			writeFileSync(prior, Buffer.from(target.before.contentsBase64 as string, "base64"), {
				mode: 0o600,
			});
		}
		recoveryRace.beforeLinkPath = secondTargetPath;
		recoveryRace.text = "second-external-edit\n";
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(readFileSync(firstTargetPath, "utf8")).toBe("first-journal-owned\n");
		expect(readFileSync(secondTargetPath, "utf8")).toBe("second-external-edit\n");
		expect(existsSync(unrelatedPath)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it.each([
		"rollback",
		"compensate",
	] as const)("resumes a target absent behind valid %s claim and prior sidecars", (direction) => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const pendingTargetPath = join(layout.dataDir, `${direction}-pending-target.txt`);
		const alreadyBeforePath = join(layout.dataDir, `${direction}-already-before.txt`);
		writeFileSync(alreadyBeforePath, "second-original\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: pendingTargetPath,
					before: storage.capabilitySetupFileState("original\n", 0o600),
					after: storage.capabilitySetupFileState("journal-owned\n", 0o600),
				},
				{
					path: alreadyBeforePath,
					before: storage.capabilitySetupFileState("second-original\n", 0o600),
					after: storage.capabilitySetupFileState("second-journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim, prior } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
			direction,
		);
		writeFileSync(claim, direction === "rollback" ? "journal-owned\n" : "original\n", {
			mode: 0o600,
		});
		writeFileSync(prior, direction === "rollback" ? "original\n" : "journal-owned\n", {
			mode: 0o600,
		});
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(storage.recoverCapabilitySetupTransaction(layout, lease)).toEqual({
				action: "rolled_back",
			});
		} finally {
			lease.close();
		}
		expect(readFileSync(pendingTargetPath, "utf8")).toBe("original\n");
		expect(readFileSync(alreadyBeforePath, "utf8")).toBe("second-original\n");
		expect(existsSync(claim)).toBe(false);
		expect(existsSync(prior)).toBe(false);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(false);
	});

	it.each([
		"rollback",
		"compensate",
	] as const)("preflights every %s sidecar before mutating a later target", (direction) => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const residueTargetPath = join(layout.dataDir, "unknown-residue-first.txt");
		const laterTargetPath = join(layout.dataDir, "must-remain-after.txt");
		writeFileSync(residueTargetPath, "first-original\n", { mode: 0o600 });
		writeFileSync(laterTargetPath, "second-journal-owned\n", { mode: 0o600 });
		const journal: storage.CapabilitySetupJournal = {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				{
					path: residueTargetPath,
					before: storage.capabilitySetupFileState("first-original\n", 0o600),
					after: storage.capabilitySetupFileState("first-journal-owned\n", 0o600),
				},
				{
					path: laterTargetPath,
					before: storage.capabilitySetupFileState("second-original\n", 0o600),
					after: storage.capabilitySetupFileState("second-journal-owned\n", 0o600),
				},
			],
		};
		storage.writeCapabilitySetupJournal(layout, journal);
		const { claim } = recoveryPaths(
			fingerprint,
			0,
			journal.targets[0] as storage.CapabilitySetupTarget,
			direction,
		);
		writeFileSync(claim, "unknown-residue\n", { mode: 0o600 });
		recoveryRace.renamedFrom.length = 0;
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(() => storage.recoverCapabilitySetupTransaction(layout, lease)).toThrow(/conflict/i);
		} finally {
			lease.close();
		}
		expect(recoveryRace.renamedFrom).not.toContain(laterTargetPath);
		expect(readFileSync(laterTargetPath, "utf8")).toBe("second-journal-owned\n");
		expect(existsSync(claim)).toBe(true);
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(true);
	});

	it("rolls back an all-after publication with a duplicate receipt target path", () => {
		const layout = tempLayout();
		storage.ensureStorageLayout(layout);
		const previousManifest = compileDefaultCapabilityManifest({
			...providerProposal(baseManifest.summaryProvider as JsonObject),
			modelId: "previous-valid-model",
		});
		const previousFingerprint = previousManifest.configurationFingerprint as string;
		const requiredIds = [
			"cli-runtime",
			"claude-mcp",
			"claude-hooks",
			"claude-hook-runtime",
			"codex-mcp",
			"codex-hooks",
			"codex-hook-runtime",
		];
		const editorTargets = requiredIds.map((id) => {
			const path = join(layout.dataDir, `${id}.txt`);
			const before = `${id}-before\n`;
			const after = `${id}-after\n`;
			writeFileSync(path, after, { mode: 0o600 });
			return {
				id,
				path,
				before,
				after,
				beforeFingerprint: createHash("sha256").update(before).digest("hex"),
				afterFingerprint: createHash("sha256").update(after).digest("hex"),
			};
		});
		const previousTargets = editorTargets.map((target) => ({
			id: target.id,
			path: target.path,
			fingerprint: target.beforeFingerprint,
		}));
		const publishedTargets = editorTargets.map((target) => ({
			id: target.id,
			path: target.path,
			fingerprint: target.afterFingerprint,
		}));
		const duplicateReceiptTargets = publishedTargets.map((target, index) =>
			index === 1
				? {
						...target,
						path: publishedTargets[0]?.path as string,
						fingerprint: publishedTargets[0]?.fingerprint as string,
					}
				: target,
		);
		const previousInstall = `${JSON.stringify({ version: 1, blocks: [], targets: previousTargets })}\n`;
		const publishedInstall = `${JSON.stringify({ version: 1, blocks: [], targets: publishedTargets })}\n`;
		const previousReceipt = `${JSON.stringify({
			version: 2,
			receiptId: "74a0c1a6-fc52-4eb0-9c28-bf346210fcbb",
			activationSequence: 4,
			configurationFingerprint: previousFingerprint,
			targets: previousTargets,
		})}\n`;
		const invalidPublishedReceipt = `${JSON.stringify({
			version: 2,
			receiptId: "0d3e6bf1-ea9b-4f48-b022-5f37c23d8928",
			activationSequence: 5,
			configurationFingerprint: fingerprint,
			targets: duplicateReceiptTargets,
		})}\n`;
		const publishedGeneration = `${JSON.stringify(baseManifest)}\n`;
		const previousGenerationPath = join(
			layout.capabilityManifestsDir,
			`${previousFingerprint}.json`,
		);
		const publishedGenerationPath = join(layout.capabilityManifestsDir, `${fingerprint}.json`);
		writeFileSync(previousGenerationPath, `${JSON.stringify(previousManifest)}\n`, { mode: 0o600 });
		writeFileSync(publishedGenerationPath, publishedGeneration, { mode: 0o600 });
		writeFileSync(layout.installManifestPath, publishedInstall, { mode: 0o600 });
		writeFileSync(layout.capabilityActivationReceiptPath, invalidPublishedReceipt, { mode: 0o600 });
		writeFileSync(layout.capabilityCurrentPointerPath, `${fingerprint}\n`, { mode: 0o600 });
		storage.writeCapabilitySetupJournal(layout, {
			version: 1,
			phase: "prepared",
			configurationFingerprint: fingerprint,
			targets: [
				...editorTargets.map((target) => ({
					path: target.path,
					before: storage.capabilitySetupFileState(target.before, 0o600),
					after: storage.capabilitySetupFileState(target.after, 0o600),
				})),
				{
					path: layout.installManifestPath,
					before: storage.capabilitySetupFileState(previousInstall, 0o600),
					after: storage.capabilitySetupFileState(publishedInstall, 0o600),
				},
				{
					path: publishedGenerationPath,
					before: storage.capabilitySetupFileState(null, null),
					after: storage.capabilitySetupFileState(publishedGeneration, 0o600),
				},
				{
					path: layout.capabilityActivationReceiptPath,
					before: storage.capabilitySetupFileState(previousReceipt, 0o600),
					after: storage.capabilitySetupFileState(invalidPublishedReceipt, 0o600),
				},
				{
					path: layout.capabilityCurrentPointerPath,
					before: storage.capabilitySetupFileState(`${previousFingerprint}\n`, 0o600),
					after: storage.capabilitySetupFileState(`${fingerprint}\n`, 0o600),
				},
			],
		});
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			expect(storage.recoverCapabilitySetupTransaction(layout, lease)).toEqual({
				action: "rolled_back",
			});
		} finally {
			lease.close();
		}
		for (const target of editorTargets) {
			expect(readFileSync(target.path, "utf8")).toBe(target.before);
		}
		expect(readFileSync(layout.installManifestPath, "utf8")).toBe(previousInstall);
		expect(readFileSync(layout.capabilityActivationReceiptPath, "utf8")).toBe(previousReceipt);
		expect(readFileSync(layout.capabilityCurrentPointerPath, "utf8")).toBe(
			`${previousFingerprint}\n`,
		);
		expect(existsSync(publishedGenerationPath)).toBe(false);
		expect(storage.readCurrentCapabilityManifest(layout)).toEqual(previousManifest);
		expect(
			storage.readValidatedCapabilityActivationReceipt(layout, previousManifest as never),
		).toMatchObject({
			configurationFingerprint: previousFingerprint,
			receiptId: "74a0c1a6-fc52-4eb0-9c28-bf346210fcbb",
			activationSequence: 4,
		});
		expect(existsSync(layout.capabilitySetupTransactionPath)).toBe(false);
	});

	it("binds the only v2 successor to an existing identical v1 base", () => {
		const layout = tempLayout();
		expect(() =>
			storage.writeCapabilityManifestGeneration(layout, fixture.outputLimitRecoveryManifest),
		).toThrow(/base generation.*missing/i);
		storage.writeCapabilityManifestGeneration(layout, baseManifest);
		const lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			storage.activateCapabilityManifest(layout, fingerprint, lease);
		} finally {
			lease.close();
		}
		expect(() =>
			storage.writeCapabilityManifestGeneration(layout, fixture.outputLimitRecoveryManifest),
		).not.toThrow();
		const unboundV1 = compileDefaultCapabilityManifest({
			...providerProposal(baseManifest.summaryProvider as JsonObject),
			modelId: "unbound-successor",
		});
		expect(() => storage.writeCapabilityManifestGeneration(layout, unboundV1)).toThrow(
			/active generation/i,
		);

		const mutated = compileCapabilityManifest({
			...manifestProposal(fixture.outputLimitRecoveryManifest),
			summaryProvider: providerProposal(
				fixture.localDerivationManifest.summaryProvider as JsonObject,
			),
		});
		expect(() => storage.writeCapabilityManifestGeneration(layout, mutated)).toThrow(
			/outside the closed output limit/i,
		);

		storage.writeCapabilityManifestGeneration(layout, fixture.localDerivationManifest);
		const successorLease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			storage.activateCapabilityManifest(
				layout,
				fixture.localDerivationManifest.configurationFingerprint as string,
				successorLease,
			);
			expect(() =>
				storage.activateCapabilityManifest(
					layout,
					fixture.outputLimitRecoveryManifest.configurationFingerprint as string,
					successorLease,
				),
			).toThrow(/active generation/i);
		} finally {
			successorLease.close();
		}
	});

	it("rejects a production v1 successor based on the test-only v2 generation", () => {
		const layout = tempLayout();
		storage.writeCapabilityManifestGeneration(layout, baseManifest);
		let lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			storage.activateCapabilityManifest(layout, fingerprint, lease);
		} finally {
			lease.close();
		}
		storage.writeCapabilityManifestGeneration(layout, fixture.outputLimitRecoveryManifest);
		lease = storage.acquireCapabilityLifecycleLock(layout, 0);
		try {
			storage.activateCapabilityManifest(
				layout,
				fixture.outputLimitRecoveryManifest.configurationFingerprint as string,
				lease,
			);
		} finally {
			lease.close();
		}
		const v1 = compileDefaultCapabilityManifest(
			{
				...providerProposal(baseManifest.summaryProvider as JsonObject),
				modelId: "v1-after-test-only-v2",
			},
			[],
			fixture.outputLimitRecoveryManifest.configurationFingerprint as string,
		);
		expect(() => storage.writeCapabilityManifestGeneration(layout, v1)).toThrow(
			/base generation is invalid/i,
		);
	});

	it("imports an activation receipt only while every target hash matches", () => {
		const layout = tempLayout();
		storage.writeCapabilityManifestGeneration(layout, baseManifest);
		const requiredIds = [
			"cli-runtime",
			"claude-mcp",
			"claude-hooks",
			"claude-hook-runtime",
			"codex-mcp",
			"codex-hooks",
			"codex-hook-runtime",
		];
		const targets = requiredIds.map((id) => {
			const path = join(layout.dataDir, `${id}.txt`);
			writeFileSync(path, `${id}\n`, { mode: 0o600 });
			return { id, path, fingerprint: storage.sha256File(path) };
		});
		writeFileSync(
			layout.installManifestPath,
			`${JSON.stringify({ version: 1, blocks: [], targets })}\n`,
			{ mode: 0o600 },
		);
		writeFileSync(
			layout.capabilityActivationReceiptPath,
			`${JSON.stringify({
				version: 1,
				configurationFingerprint: fingerprint,
				targets: targets.slice(1),
			})}\n`,
			{ mode: 0o600 },
		);
		expect(() =>
			storage.readValidatedCapabilityActivationReceipt(layout, baseManifest as never),
		).toThrow(/install manifest/i);
		writeFileSync(
			layout.capabilityActivationReceiptPath,
			`${JSON.stringify({
				version: 1,
				configurationFingerprint: fingerprint,
				targets,
			})}\n`,
			{ mode: 0o600 },
		);
		expect(
			storage.readValidatedCapabilityActivationReceipt(layout, baseManifest as never),
		).toMatchObject({ version: 1, configurationFingerprint: fingerprint });
		writeFileSync(
			layout.capabilityActivationReceiptPath,
			`${JSON.stringify({
				version: 2,
				receiptId: "74A0C1A6-FC52-4EB0-9C28-BF346210FCBB",
				activationSequence: 1,
				configurationFingerprint: fingerprint,
				targets,
			})}\n`,
			{ mode: 0o600 },
		);
		expect(
			storage.readValidatedCapabilityActivationReceipt(layout, baseManifest as never),
		).toMatchObject({
			version: 2,
			receiptId: "74a0c1a6-fc52-4eb0-9c28-bf346210fcbb",
			activationSequence: 1,
		});
		const compatibilityPath = join(layout.dataDir, "opencode.jsonc");
		writeFileSync(compatibilityPath, '{"mcp":{}}\n', { mode: 0o600 });
		const compatibilityTarget = {
			id: "opencode-mcp",
			path: compatibilityPath,
			fingerprint: storage.sha256File(compatibilityPath),
		};
		writeFileSync(
			layout.installManifestPath,
			`${JSON.stringify({
				version: 1,
				blocks: [],
				targets: [...targets, compatibilityTarget],
			})}\n`,
			{ mode: 0o600 },
		);
		expect(
			storage.readValidatedCapabilityActivationReceipt(layout, baseManifest as never),
		).toMatchObject({ configurationFingerprint: fingerprint, targets });
		writeFileSync(compatibilityPath, '{"mcp":{"external":true}}\n', { mode: 0o600 });
		expect(
			storage.readValidatedCapabilityActivationReceipt(layout, baseManifest as never),
		).toMatchObject({ configurationFingerprint: fingerprint, targets });

		const otherCompatibilityPath = join(layout.dataDir, "opencode-plugin.js");
		writeFileSync(otherCompatibilityPath, "export {};\n", { mode: 0o600 });
		const otherCompatibilityTarget = {
			id: "opencode-plugin",
			path: otherCompatibilityPath,
			fingerprint: storage.sha256File(otherCompatibilityPath),
		};
		for (const [name, compatibilityTargets] of [
			["malformed", [{ ...compatibilityTarget, fingerprint: "not-a-sha256" }]],
			[
				"duplicate id",
				[compatibilityTarget, { ...otherCompatibilityTarget, id: compatibilityTarget.id }],
			],
			[
				"duplicate extra path",
				[compatibilityTarget, { ...otherCompatibilityTarget, path: compatibilityTarget.path }],
			],
			[
				"required path conflict",
				[{ ...compatibilityTarget, path: targets[0]?.path, fingerprint: targets[0]?.fingerprint }],
			],
		] as const) {
			writeFileSync(
				layout.installManifestPath,
				`${JSON.stringify({
					version: 1,
					blocks: [],
					targets: [...targets, ...compatibilityTargets],
				})}\n`,
				{ mode: 0o600 },
			);
			expect(
				() => storage.readValidatedCapabilityActivationReceipt(layout, baseManifest as never),
				name,
			).toThrow(/install manifest target inventory/i);
		}

		writeFileSync(
			layout.installManifestPath,
			`${JSON.stringify({ version: 1, blocks: [], targets })}\n`,
			{ mode: 0o600 },
		);
		writeFileSync(targets[0]?.path as string, "external edit\n", { mode: 0o600 });
		expect(() =>
			storage.readValidatedCapabilityActivationReceipt(layout, baseManifest as never),
		).toThrow(/target hash mismatch/i);
	});
});
