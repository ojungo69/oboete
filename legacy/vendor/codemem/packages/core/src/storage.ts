import { createHash } from "node:crypto";
import {
	chmodSync,
	closeSync,
	existsSync,
	lstatSync,
	openSync,
	readFileSync,
	readlinkSync,
	readSync,
} from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import { isDeepStrictEqual } from "node:util";
import {
	type EffectiveCapabilityManifestV1,
	validateCapabilityManifest,
} from "./capability-manifest.js";
import type { StorageLayout } from "./storage-layout.js";
import {
	type DurableRestoreFileInput,
	durableClearRestoreSidecarsIfKnown,
	durableCopyFile,
	durableRemoveFile,
	durableReplaceFile,
	durableReplaceSymlink,
	durableRestoreCanResume,
	durableRestoreFileIfUnchanged,
	ensurePrivateDirectory,
	fsyncPath,
	inspectDurableRestoreSidecars,
	inspectRegularFile,
	MAX_CAPABILITY_SETUP_FILE_BYTES,
} from "./storage-platform.js";
import { ReadOnlyActor, WriterActor } from "./writer-actor.js";

export { DEFAULT_DATA_DIR, resolveRuntimeDataDir, resolveStorageLayout } from "./storage-layout.js";
export type { StorageLayout };

export type StorageJournalState = "prepared" | "switched" | "committed";

export interface StorageJournal {
	version: 1;
	operationId: string;
	state: StorageJournalState;
	oldPointer: string | null;
	newPointer: string;
	artifactSha256: string;
}

export interface CapabilityLifecycleLease {
	close(): void;
}

export interface CapabilitySetupFileState {
	contentsBase64: string | null;
	mode: number | null;
	sha256: string | null;
}

export interface CapabilitySetupTarget {
	path: string;
	before: CapabilitySetupFileState;
	after: CapabilitySetupFileState;
}

export interface CapabilitySetupJournal {
	version: 1;
	phase: "prepared";
	configurationFingerprint: string;
	targets: CapabilitySetupTarget[];
}

type CapabilityActivationReceiptTarget = {
	id: string;
	path: string;
	fingerprint: string;
};

export type CapabilityActivationReceipt =
	| {
			version: 1;
			configurationFingerprint: string;
			targets: CapabilityActivationReceiptTarget[];
	  }
	| {
			version: 2;
			receiptId: string;
			activationSequence: number;
			configurationFingerprint: string;
			targets: CapabilityActivationReceiptTarget[];
	  };

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

const OPERATION_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const ARTIFACT_POINTER = /^versions\/[A-Za-z0-9][A-Za-z0-9._-]{0,127}\.sqlite$/;
const SHA256 = /^[a-f0-9]{64}$/;
const CAPABILITY_FINGERPRINT = /^sha256:[a-f0-9]{64}$/;
const MAX_CAPABILITY_SETUP_FILE_BASE64_CHARS = (MAX_CAPABILITY_SETUP_FILE_BYTES / 3) * 4;
const MAX_CAPABILITY_SETUP_TOTAL_STATE_BASE64_CHARS = 24_000_000;
const MAX_CAPABILITY_SETUP_JOURNAL_BYTES = 32 * 1024 * 1024;
const MAX_CAPABILITY_CURRENT_POINTER_BYTES = 128;
const MAX_CAPABILITY_MANIFEST_GENERATION_BYTES = 1024 * 1024;
const CAPABILITY_ACTIVATION_TARGET_IDS = [
	"cli-runtime",
	"claude-mcp",
	"claude-hooks",
	"claude-hook-runtime",
	"codex-mcp",
	"codex-hooks",
	"codex-hook-runtime",
] as const;
const activeCapabilityLeases = new WeakMap<CapabilityLifecycleLease, string>();

export function ensureStorageLayout(layout: StorageLayout): void {
	ensurePrivateDirectory(layout.dataDir);
	ensurePrivateDirectory(layout.controlDir);
	ensurePrivateDirectory(layout.capabilitiesDir);
	ensurePrivateDirectory(layout.capabilityManifestsDir);
	ensurePrivateDirectory(layout.dbDir);
	ensurePrivateDirectory(layout.versionsDir);
	ensurePrivateDirectory(layout.spoolDir);
	ensurePrivateDirectory(layout.backupsDir);
}

function validateCapabilityFingerprint(fingerprint: string): void {
	if (!CAPABILITY_FINGERPRINT.test(fingerprint)) {
		throw new Error("Invalid capability manifest fingerprint.");
	}
}

function capabilityManifestPath(layout: StorageLayout, fingerprint: string): string {
	validateCapabilityFingerprint(fingerprint);
	return join(layout.capabilityManifestsDir, `${fingerprint}.json`);
}

function readUtf8FileStrict(path: string, maxBytes: number): string {
	const inspected = inspectRegularFile(path, maxBytes);
	if (inspected.state !== "regular") {
		throw new Error(`Capability storage path is not a regular file: ${path}`);
	}
	try {
		return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(inspected.contents);
	} catch (error) {
		throw new Error(`Capability storage file is not valid UTF-8: ${path}`, { cause: error });
	}
}

export function writeCapabilityManifestGeneration(layout: StorageLayout, manifest: unknown): void {
	ensureStorageLayout(layout);
	const validated = validateCapabilityManifest(manifest);
	assertCapabilityActivationBase(layout, validated);
	const path = capabilityManifestPath(layout, validated.configurationFingerprint);
	const contents = `${JSON.stringify(validated)}\n`;
	if (existsSync(path)) {
		if (readUtf8FileStrict(path, MAX_CAPABILITY_MANIFEST_GENERATION_BYTES) === contents) return;
		throw new Error("Capability manifest generation is immutable.");
	}
	durableReplaceFile(path, contents);
}

export function readCapabilityManifestGeneration(
	layout: StorageLayout,
	fingerprint: string,
): EffectiveCapabilityManifestV1 {
	const path = capabilityManifestPath(layout, fingerprint);
	if (!existsSync(path)) throw new Error("Capability manifest generation is missing.");
	let parsed: unknown;
	try {
		parsed = JSON.parse(readUtf8FileStrict(path, MAX_CAPABILITY_MANIFEST_GENERATION_BYTES));
	} catch (error) {
		throw new Error("Capability manifest generation is malformed.", { cause: error });
	}
	const manifest = validateCapabilityManifest(parsed);
	if (manifest.configurationFingerprint !== fingerprint) {
		throw new Error("Capability manifest generation does not match its fingerprint path.");
	}
	validateCapabilityBaseGeneration(layout, manifest);
	return manifest;
}

function validateCapabilityBaseGeneration(
	layout: StorageLayout,
	manifest: EffectiveCapabilityManifestV1,
): void {
	const baseFingerprint = manifest.baseConfigurationFingerprint;
	if (!baseFingerprint) {
		if (manifest.resourceProfile.version === 2) {
			throw new Error("Capability successor has no base generation.");
		}
		return;
	}
	const basePath = capabilityManifestPath(layout, baseFingerprint);
	if (!existsSync(basePath)) throw new Error("Capability successor base generation is missing.");
	let baseValue: unknown;
	try {
		baseValue = JSON.parse(readUtf8FileStrict(basePath, MAX_CAPABILITY_MANIFEST_GENERATION_BYTES));
	} catch (error) {
		throw new Error("Capability successor base generation is malformed.", { cause: error });
	}
	const base = validateCapabilityManifest(baseValue);
	if (base.configurationFingerprint !== baseFingerprint) {
		throw new Error("Capability successor base generation is invalid.");
	}
	if (base.resourceProfile.version !== 1) {
		throw new Error("Capability successor base generation is invalid.");
	}
	if (manifest.resourceProfile.version !== 2) return;
	const {
		version: _baseVersion,
		maxMemoryItemsPerDerivation: _baseLimit,
		...baseProfile
	} = base.resourceProfile;
	const {
		version: _nextVersion,
		maxMemoryItemsPerDerivation: _nextLimit,
		...nextProfile
	} = manifest.resourceProfile;
	if (
		!isDeepStrictEqual(baseProfile, nextProfile) ||
		!isDeepStrictEqual(base.destinationPolicyMap, manifest.destinationPolicyMap) ||
		!isDeepStrictEqual(base.summaryProvider, manifest.summaryProvider) ||
		!isDeepStrictEqual(base.embeddingProvider, manifest.embeddingProvider) ||
		!isDeepStrictEqual(base.legacyDispositions, manifest.legacyDispositions)
	) {
		throw new Error("Capability successor changes fields outside the closed output limit.");
	}
}

export function assertCapabilityActivationBase(
	layout: StorageLayout,
	manifest: EffectiveCapabilityManifestV1,
): void {
	validateCapabilityBaseGeneration(layout, manifest);
	const current = readCurrentCapabilityManifest(layout);
	if (current?.configurationFingerprint === manifest.configurationFingerprint) return;
	if (current === null) {
		if (manifest.baseConfigurationFingerprint) {
			throw new Error("Initial capability activation cannot declare a predecessor.");
		}
		return;
	}
	if (manifest.baseConfigurationFingerprint !== current.configurationFingerprint) {
		throw new Error("Capability successor does not reference the active generation.");
	}
}

export function acquireCapabilityLifecycleLock(
	layout: StorageLayout,
	deadlineMs = 5_000,
): CapabilityLifecycleLease {
	ensureStorageLayout(layout);
	const lock = WriterActor.open(layout.capabilityLifecycleLockPath);
	try {
		chmodSync(layout.capabilityLifecycleLockPath, 0o600);
		lock.pragma("journal_mode = DELETE");
		lock.pragma(`busy_timeout = ${Math.max(0, Math.trunc(deadlineMs))}`);
		lock.exec("BEGIN IMMEDIATE");
	} catch (error) {
		lock.close();
		const message = error instanceof Error ? error.message : String(error);
		if (/busy|locked|SQLITE_BUSY/i.test(message)) {
			throw new Error("Capability lifecycle lock is busy.");
		}
		throw error;
	}
	let open = true;
	const lease: CapabilityLifecycleLease = {
		close(): void {
			if (!open) return;
			open = false;
			activeCapabilityLeases.delete(lease);
			try {
				lock.exec("ROLLBACK");
			} finally {
				lock.close();
			}
		},
	};
	activeCapabilityLeases.set(lease, layout.capabilityLifecycleLockPath);
	return lease;
}

function assertCapabilityLifecycleLease(
	layout: StorageLayout,
	lease: CapabilityLifecycleLease | undefined,
): asserts lease is CapabilityLifecycleLease {
	if (!lease || activeCapabilityLeases.get(lease) !== layout.capabilityLifecycleLockPath) {
		throw new Error("An active capability lifecycle lease is required.");
	}
}

export function activateCapabilityManifest(
	layout: StorageLayout,
	fingerprint: string,
	lease?: CapabilityLifecycleLease,
): void {
	assertCapabilityLifecycleLease(layout, lease);
	const manifest = readCapabilityManifestGeneration(layout, fingerprint);
	assertCapabilityActivationBase(layout, manifest);
	if (readCurrentCapabilityManifest(layout)?.configurationFingerprint === fingerprint) return;
	durableReplaceFile(layout.capabilityCurrentPointerPath, `${fingerprint}\n`);
}

export function readCurrentCapabilityManifest(
	layout: StorageLayout,
): EffectiveCapabilityManifestV1 | null {
	if (!existsSync(layout.capabilityCurrentPointerPath)) return null;
	const contents = readUtf8FileStrict(
		layout.capabilityCurrentPointerPath,
		MAX_CAPABILITY_CURRENT_POINTER_BYTES,
	);
	const fingerprint = contents.endsWith("\n") ? contents.slice(0, -1) : contents;
	if (fingerprint.includes("\n")) throw new Error("Capability current pointer is malformed.");
	validateCapabilityFingerprint(fingerprint);
	return readCapabilityManifestGeneration(layout, fingerprint);
}

export function capabilitySetupFileState(
	contents: Uint8Array | string | null,
	mode: number | null,
): CapabilitySetupFileState {
	if (contents === null) return { contentsBase64: null, mode: null, sha256: null };
	if (!Number.isInteger(mode) || mode === null || mode < 0 || mode > 0o777) {
		throw new Error("Capability setup target mode is invalid.");
	}
	const bytes =
		typeof contents === "string" ? Buffer.from(contents, "utf8") : Buffer.from(contents);
	return {
		contentsBase64: bytes.toString("base64"),
		mode,
		sha256: createHash("sha256").update(bytes).digest("hex"),
	};
}

function hasOnlyKeys(value: object, expected: readonly string[]): boolean {
	const keys = Object.keys(value);
	return keys.length === expected.length && keys.every((key) => expected.includes(key));
}

function validateCapabilitySetupFileState(value: unknown): CapabilitySetupFileState {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new Error("Capability setup file state is malformed.");
	}
	if (!hasOnlyKeys(value, ["contentsBase64", "mode", "sha256"])) {
		throw new Error("Capability setup file state has unsupported fields.");
	}
	const state = value as Partial<CapabilitySetupFileState>;
	if (state.contentsBase64 === null) {
		if (state.mode !== null || state.sha256 !== null) {
			throw new Error("Absent capability setup state has file metadata.");
		}
		return { contentsBase64: null, mode: null, sha256: null };
	}
	if (
		typeof state.contentsBase64 !== "string" ||
		state.contentsBase64.length > MAX_CAPABILITY_SETUP_FILE_BASE64_CHARS ||
		!Number.isInteger(state.mode) ||
		(state.mode as number) < 0 ||
		(state.mode as number) > 0o777 ||
		typeof state.sha256 !== "string" ||
		!SHA256.test(state.sha256)
	) {
		throw new Error("Capability setup file state is malformed.");
	}
	const bytes = Buffer.from(state.contentsBase64, "base64");
	if (
		bytes.toString("base64") !== state.contentsBase64 ||
		createHash("sha256").update(bytes).digest("hex") !== state.sha256
	) {
		throw new Error("Capability setup file state hash is invalid.");
	}
	return {
		contentsBase64: state.contentsBase64,
		mode: state.mode as number,
		sha256: state.sha256,
	};
}

function validateCapabilitySetupJournal(value: unknown): CapabilitySetupJournal {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new Error("Capability setup journal is malformed.");
	}
	if (!hasOnlyKeys(value, ["version", "phase", "configurationFingerprint", "targets"])) {
		throw new Error("Capability setup journal has unsupported fields.");
	}
	const journal = value as Partial<CapabilitySetupJournal>;
	if (
		journal.version !== 1 ||
		journal.phase !== "prepared" ||
		typeof journal.configurationFingerprint !== "string" ||
		!CAPABILITY_FINGERPRINT.test(journal.configurationFingerprint) ||
		!Array.isArray(journal.targets) ||
		journal.targets.length < 1 ||
		journal.targets.length > 32
	) {
		throw new Error("Capability setup journal is malformed.");
	}
	const seen = new Set<string>();
	let totalStateBase64Chars = 0;
	const targets = journal.targets.map((target) => {
		if (!target || typeof target !== "object" || Array.isArray(target)) {
			throw new Error("Capability setup journal target is malformed.");
		}
		if (!hasOnlyKeys(target, ["path", "before", "after"])) {
			throw new Error("Capability setup journal target has unsupported fields.");
		}
		const candidate = target as Partial<CapabilitySetupTarget>;
		const path = typeof candidate.path === "string" ? resolve(candidate.path) : "";
		if (typeof candidate.path !== "string" || !isAbsolute(candidate.path) || seen.has(path)) {
			throw new Error("Capability setup journal target path is invalid.");
		}
		seen.add(path);
		for (const state of [candidate.before, candidate.after]) {
			if (state && typeof state === "object" && !Array.isArray(state)) {
				const contentsBase64 = (state as Partial<CapabilitySetupFileState>).contentsBase64;
				if (typeof contentsBase64 === "string") {
					totalStateBase64Chars += contentsBase64.length;
					if (totalStateBase64Chars > MAX_CAPABILITY_SETUP_TOTAL_STATE_BASE64_CHARS) {
						throw new Error("Capability setup journal state budget is exceeded.");
					}
				}
			}
		}
		return {
			path,
			before: validateCapabilitySetupFileState(candidate.before),
			after: validateCapabilitySetupFileState(candidate.after),
		};
	});
	return {
		version: 1,
		phase: "prepared",
		configurationFingerprint: journal.configurationFingerprint,
		targets,
	};
}

export function writeCapabilitySetupJournal(
	layout: StorageLayout,
	journal: CapabilitySetupJournal,
): void {
	ensureStorageLayout(layout);
	const validated = validateCapabilitySetupJournal(journal);
	const contents = `${JSON.stringify(validated)}\n`;
	if (Buffer.byteLength(contents) > MAX_CAPABILITY_SETUP_JOURNAL_BYTES) {
		throw new Error("Capability setup journal byte budget is exceeded.");
	}
	durableReplaceFile(layout.capabilitySetupTransactionPath, contents);
}

function readCapabilitySetupJournal(layout: StorageLayout): CapabilitySetupJournal | null {
	const state = inspectRegularFile(
		layout.capabilitySetupTransactionPath,
		MAX_CAPABILITY_SETUP_JOURNAL_BYTES,
	);
	if (state.state === "absent") return null;
	if (state.state !== "regular" || state.mode !== 0o600) {
		throw new Error("Capability setup journal is not an owner-only regular file.");
	}
	let parsed: unknown;
	try {
		parsed = JSON.parse(
			new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(state.contents),
		);
		return validateCapabilitySetupJournal(parsed);
	} catch (error) {
		throw new Error("Capability setup journal recovery failed.", { cause: error });
	}
}

function readCapabilitySetupTargetState(path: string): CapabilitySetupFileState | null {
	const inspected = inspectRegularFile(path);
	if (inspected.state === "absent") return capabilitySetupFileState(null, null);
	return inspected.state === "regular"
		? capabilitySetupFileState(inspected.contents, inspected.mode)
		: null;
}

export function readValidatedCapabilityActivationReceipt(
	layout: StorageLayout,
	manifest: EffectiveCapabilityManifestV1,
): CapabilityActivationReceipt | null {
	if (!existsSync(layout.capabilityActivationReceiptPath)) return null;
	const receiptState = readCapabilitySetupTargetState(layout.capabilityActivationReceiptPath);
	if (receiptState?.mode !== 0o600) {
		throw new Error("Capability activation receipt is not an owner-only regular file.");
	}
	let parsed: unknown;
	try {
		parsed = JSON.parse(capabilitySetupStateText(receiptState) ?? "");
	} catch (error) {
		throw new Error("Capability activation receipt is malformed.", { cause: error });
	}
	if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
		throw new Error("Capability activation receipt is malformed.");
	}
	const value = parsed as {
		version?: unknown;
		receiptId?: unknown;
		activationSequence?: unknown;
		configurationFingerprint?: unknown;
		targets?: unknown;
	};
	const isV1 = value.version === 1;
	const isV2 = value.version === 2;
	if (
		(!isV1 && !isV2) ||
		!hasOnlyKeys(
			parsed,
			isV2
				? ["version", "receiptId", "activationSequence", "configurationFingerprint", "targets"]
				: ["version", "configurationFingerprint", "targets"],
		) ||
		(isV2 &&
			(typeof value.receiptId !== "string" ||
				!UUID.test(value.receiptId) ||
				!Number.isSafeInteger(value.activationSequence) ||
				(value.activationSequence as number) < 1)) ||
		value.configurationFingerprint !== manifest.configurationFingerprint ||
		!Array.isArray(value.targets) ||
		value.targets.length < 1 ||
		value.targets.length > 16
	) {
		throw new Error("Capability activation receipt does not match the active manifest.");
	}
	const ids = new Set<string>();
	const paths = new Set<string>();
	const targets = value.targets.map((target) => {
		if (
			!target ||
			typeof target !== "object" ||
			Array.isArray(target) ||
			!hasOnlyKeys(target, ["id", "path", "fingerprint"])
		) {
			throw new Error("Capability activation receipt target is malformed.");
		}
		const candidate = target as { id?: unknown; path?: unknown; fingerprint?: unknown };
		if (
			typeof candidate.id !== "string" ||
			!OPERATION_ID.test(candidate.id) ||
			typeof candidate.path !== "string" ||
			!isAbsolute(candidate.path) ||
			typeof candidate.fingerprint !== "string" ||
			!SHA256.test(candidate.fingerprint) ||
			ids.has(candidate.id) ||
			paths.has(candidate.path)
		) {
			throw new Error("Capability activation receipt target is invalid.");
		}
		ids.add(candidate.id);
		paths.add(candidate.path);
		const current = readCapabilitySetupTargetState(candidate.path);
		if (current?.sha256 !== candidate.fingerprint) {
			throw new Error("Capability activation receipt target hash mismatch.");
		}
		return {
			id: candidate.id,
			path: candidate.path,
			fingerprint: candidate.fingerprint,
		};
	});
	const installState = readCapabilitySetupTargetState(layout.installManifestPath);
	if (installState?.mode !== 0o600) {
		throw new Error("Capability install manifest is not an owner-only regular file.");
	}
	let installValue: unknown;
	try {
		installValue = JSON.parse(capabilitySetupStateText(installState) ?? "");
	} catch (error) {
		throw new Error("Capability install manifest is malformed.", { cause: error });
	}
	if (!installValue || typeof installValue !== "object" || Array.isArray(installValue)) {
		throw new Error("Capability install manifest is malformed.");
	}
	const install = installValue as { version?: unknown; blocks?: unknown; targets?: unknown };
	if (
		!hasOnlyKeys(installValue, ["version", "blocks", "targets"]) ||
		install.version !== 1 ||
		!Array.isArray(install.blocks) ||
		install.blocks.length !== 0 ||
		!Array.isArray(install.targets) ||
		install.targets.length < CAPABILITY_ACTIVATION_TARGET_IDS.length ||
		install.targets.length > 16
	) {
		throw new Error("Capability install manifest is malformed.");
	}
	const installIds = new Set<string>();
	const installPaths = new Set<string>();
	const installTargets = (install.targets as Array<Record<string, unknown>>).map((target) => {
		if (
			!target ||
			typeof target !== "object" ||
			Array.isArray(target) ||
			!hasOnlyKeys(target, ["id", "path", "fingerprint"]) ||
			typeof target.id !== "string" ||
			!OPERATION_ID.test(target.id) ||
			typeof target.path !== "string" ||
			!isAbsolute(target.path) ||
			typeof target.fingerprint !== "string" ||
			!SHA256.test(target.fingerprint) ||
			installIds.has(target.id) ||
			installPaths.has(resolve(target.path))
		) {
			throw new Error("Capability install manifest target inventory is invalid.");
		}
		installIds.add(target.id);
		installPaths.add(resolve(target.path));
		return {
			id: target.id,
			path: target.path,
			fingerprint: target.fingerprint,
		};
	});
	const requiredReceiptIds = new Set<string>(CAPABILITY_ACTIVATION_TARGET_IDS);
	if (
		targets.length !== CAPABILITY_ACTIVATION_TARGET_IDS.length ||
		targets.some((target) => !requiredReceiptIds.delete(target.id)) ||
		requiredReceiptIds.size !== 0
	) {
		throw new Error("Capability activation receipt does not match the install manifest.");
	}
	const installById = new Map(installTargets.map((target) => [target.id, target]));
	if (targets.some((target) => !isDeepStrictEqual(target, installById.get(target.id)))) {
		throw new Error("Capability install manifest target inventory is invalid.");
	}
	return isV2
		? {
				version: 2,
				receiptId: (value.receiptId as string).toLowerCase(),
				activationSequence: value.activationSequence as number,
				configurationFingerprint: manifest.configurationFingerprint,
				targets,
			}
		: {
				version: 1,
				configurationFingerprint: manifest.configurationFingerprint,
				targets,
			};
}

function sameCapabilitySetupState(
	left: CapabilitySetupFileState,
	right: CapabilitySetupFileState,
): boolean {
	return left.mode === right.mode && left.sha256 === right.sha256;
}

function capabilitySetupStateText(state: CapabilitySetupFileState): string | null {
	if (state.contentsBase64 === null) return null;
	try {
		return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
			Buffer.from(state.contentsBase64, "base64"),
		);
	} catch {
		return null;
	}
}

function validatePublishedCapabilityBinding(
	layout: StorageLayout,
	journal: CapabilitySetupJournal,
): void {
	const target = (path: string) =>
		journal.targets.find((candidate) => candidate.path === path)?.after;
	const current = target(layout.capabilityCurrentPointerPath);
	const receipt = target(layout.capabilityActivationReceiptPath);
	const generationPath = capabilityManifestPath(layout, journal.configurationFingerprint);
	const generation = target(generationPath);
	const currentText = current ? capabilitySetupStateText(current) : null;
	if (currentText !== `${journal.configurationFingerprint}\n`) {
		throw new Error("Capability setup journal current binding is invalid.");
	}
	let receiptValue: unknown;
	let generationValue: unknown;
	try {
		receiptValue = JSON.parse(receipt ? (capabilitySetupStateText(receipt) ?? "") : "");
		generationValue = JSON.parse(generation ? (capabilitySetupStateText(generation) ?? "") : "");
	} catch (error) {
		throw new Error("Capability setup journal publication binding is malformed.", { cause: error });
	}
	if (
		!receiptValue ||
		typeof receiptValue !== "object" ||
		Array.isArray(receiptValue) ||
		(receiptValue as Record<string, unknown>).configurationFingerprint !==
			journal.configurationFingerprint
	) {
		throw new Error("Capability setup journal receipt binding is invalid.");
	}
	const manifest = validateCapabilityManifest(generationValue);
	if (manifest.configurationFingerprint !== journal.configurationFingerprint) {
		throw new Error("Capability setup journal generation binding is invalid.");
	}
	if (!readValidatedCapabilityActivationReceipt(layout, manifest)) {
		throw new Error("Capability setup journal activation receipt is missing.");
	}
}

function capabilitySetupTransitionInput(
	journal: CapabilitySetupJournal,
	target: CapabilitySetupTarget,
	index: number,
	direction: "rollback" | "compensate",
): DurableRestoreFileInput {
	const from = direction === "rollback" ? target.after : target.before;
	const to = direction === "rollback" ? target.before : target.after;
	const recoveryId = createHash("sha256")
		.update(
			JSON.stringify([
				journal.configurationFingerprint,
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
	return {
		path: target.path,
		recoveryId,
		expectedAfter: from.sha256 === null ? null : { sha256: from.sha256, mode: from.mode as number },
		restoreBefore:
			to.contentsBase64 === null
				? null
				: {
						contents: Buffer.from(to.contentsBase64, "base64"),
						mode: to.mode as number,
					},
	};
}

function transitionCapabilitySetupTarget(
	journal: CapabilitySetupJournal,
	target: CapabilitySetupTarget,
	index: number,
	direction: "rollback" | "compensate",
): void {
	const result = durableRestoreFileIfUnchanged(
		capabilitySetupTransitionInput(journal, target, index, direction),
	);
	if (result === "conflict") throw new Error("Capability setup journal recovery conflict.");
}

export function recoverCapabilitySetupTransaction(
	layout: StorageLayout,
	lease?: CapabilityLifecycleLease,
): {
	action: "none" | "completed" | "rolled_back";
} {
	assertCapabilityLifecycleLease(layout, lease);
	const journal = readCapabilitySetupJournal(layout);
	if (!journal) return { action: "none" };
	const recoveries = journal.targets.flatMap((target, index) =>
		sameCapabilitySetupState(target.before, target.after)
			? []
			: (["rollback", "compensate"] as const).map((direction) => ({
					index,
					direction,
					input: capabilitySetupTransitionInput(journal, target, index, direction),
				})),
	);
	const inspectedRecoveries = recoveries.map((recovery) => ({
		...recovery,
		sidecars: inspectDurableRestoreSidecars(recovery.input),
	}));
	if (inspectedRecoveries.some((recovery) => recovery.sidecars === "conflict")) {
		throw new Error("Capability setup journal recovery conflict.");
	}
	if (
		journal.targets.some(
			(_, index) =>
				inspectedRecoveries.filter(
					(recovery) => recovery.index === index && recovery.sidecars === "recoverable",
				).length > 1,
		)
	) {
		throw new Error("Capability setup journal recovery conflict.");
	}
	const initial = journal.targets.map((target) => readCapabilitySetupTargetState(target.path));
	if (initial.includes(null)) {
		throw new Error("Capability setup journal recovery conflict.");
	}
	const resumptions = (initial as CapabilitySetupFileState[]).flatMap((state, index) => {
		const target = journal.targets[index] as CapabilitySetupTarget;
		if (
			sameCapabilitySetupState(state, target.before) ||
			sameCapabilitySetupState(state, target.after)
		) {
			return [];
		}
		const candidates = inspectedRecoveries.filter(
			(recovery) =>
				recovery.index === index &&
				recovery.sidecars === "recoverable" &&
				durableRestoreCanResume(recovery.input),
		);
		if (candidates.length !== 1) {
			throw new Error("Capability setup journal recovery conflict.");
		}
		return candidates;
	});
	const resumeStates = journal.targets.map((target) => readCapabilitySetupTargetState(target.path));
	if (
		resumeStates.some(
			(state, index) =>
				state === null ||
				!sameCapabilitySetupState(state, initial[index] as CapabilitySetupFileState),
		)
	) {
		throw new Error("Capability setup journal recovery conflict.");
	}
	const completedResumptions: typeof resumptions = [];
	const compensateCompletedResumptions = (skipIndexes: ReadonlySet<number> = new Set()) => {
		for (const recovery of [...completedResumptions].reverse()) {
			if (skipIndexes.has(recovery.index)) continue;
			try {
				transitionCapabilitySetupTarget(
					journal,
					journal.targets[recovery.index] as CapabilitySetupTarget,
					recovery.index,
					recovery.direction === "rollback" ? "compensate" : "rollback",
				);
			} catch {
				// A concurrent external save wins; the journal remains for explicit recovery.
			}
		}
	};
	try {
		for (const recovery of resumptions) {
			if (durableRestoreFileIfUnchanged(recovery.input) === "conflict") {
				throw new Error("Capability setup journal recovery conflict.");
			}
			completedResumptions.push(recovery);
		}
	} catch (error) {
		compensateCompletedResumptions();
		throw error;
	}
	let states: CapabilitySetupFileState[];
	let rollbackStates: CapabilitySetupFileState[];
	try {
		const observedStates = journal.targets.map((target) =>
			readCapabilitySetupTargetState(target.path),
		);
		if (
			observedStates.some((state, index) => {
				if (state === null) return true;
				const target = journal.targets[index] as CapabilitySetupTarget;
				return (
					!sameCapabilitySetupState(state, target.before) &&
					!sameCapabilitySetupState(state, target.after)
				);
			})
		) {
			throw new Error("Capability setup journal recovery conflict.");
		}
		states = observedStates as CapabilitySetupFileState[];
		const remainingRecoveries = recoveries.map((recovery) => ({
			...recovery,
			sidecars: inspectDurableRestoreSidecars(recovery.input),
		}));
		if (remainingRecoveries.some((recovery) => recovery.sidecars === "conflict")) {
			throw new Error("Capability setup journal recovery conflict.");
		}
		if (
			journal.targets.some(
				(_, index) =>
					remainingRecoveries.filter(
						(recovery) => recovery.index === index && recovery.sidecars === "recoverable",
					).length > 1,
			)
		) {
			throw new Error("Capability setup journal recovery conflict.");
		}
		if (
			remainingRecoveries.some((recovery) => !durableClearRestoreSidecarsIfKnown(recovery.input))
		) {
			throw new Error("Capability setup journal recovery conflict.");
		}
		const observedRollbackStates = journal.targets.map((target) =>
			readCapabilitySetupTargetState(target.path),
		);
		if (
			observedRollbackStates.some(
				(state, index) =>
					state === null ||
					!sameCapabilitySetupState(state, states[index] as CapabilitySetupFileState),
			)
		) {
			throw new Error("Capability setup journal recovery conflict.");
		}
		rollbackStates = observedRollbackStates as CapabilitySetupFileState[];
		const allAfter = states.every((state, index) =>
			sameCapabilitySetupState(state, journal.targets[index]?.after as CapabilitySetupFileState),
		);
		if (allAfter) {
			if (
				journal.targets.some((target, index) => {
					const latest = readCapabilitySetupTargetState(target.path);
					return (
						latest === null ||
						!sameCapabilitySetupState(latest, rollbackStates[index] as CapabilitySetupFileState)
					);
				})
			) {
				throw new Error("Capability setup journal recovery conflict.");
			}
			let publicationValid = true;
			try {
				validatePublishedCapabilityBinding(layout, journal);
			} catch {
				publicationValid = false;
			}
			if (publicationValid) {
				durableRemoveFile(layout.capabilitySetupTransactionPath);
				return { action: "completed" };
			}
		}
	} catch (error) {
		compensateCompletedResumptions();
		throw error;
	}
	const restored: number[] = [];
	try {
		for (let index = journal.targets.length - 1; index >= 0; index--) {
			const target = journal.targets[index] as CapabilitySetupTarget;
			const state = rollbackStates[index] as CapabilitySetupFileState;
			if (!sameCapabilitySetupState(target.before, target.after)) {
				const latest = readCapabilitySetupTargetState(target.path);
				if (!latest || !sameCapabilitySetupState(latest, state)) {
					throw new Error("Capability setup journal recovery conflict.");
				}
				transitionCapabilitySetupTarget(journal, target, index, "rollback");
				if (sameCapabilitySetupState(state, target.after)) restored.push(index);
			}
		}
		if (
			journal.targets.some((target) => {
				const state = readCapabilitySetupTargetState(target.path);
				return state === null || !sameCapabilitySetupState(state, target.before);
			})
		) {
			throw new Error("Capability setup journal recovery conflict.");
		}
	} catch (error) {
		const restoredIndexes = new Set(restored);
		for (const index of [...restored].reverse()) {
			try {
				transitionCapabilitySetupTarget(
					journal,
					journal.targets[index] as CapabilitySetupTarget,
					index,
					"compensate",
				);
			} catch {
				// A concurrent external save wins; the journal remains for explicit recovery.
			}
		}
		compensateCompletedResumptions(restoredIndexes);
		throw error;
	}
	durableRemoveFile(layout.capabilitySetupTransactionPath);
	return { action: "rolled_back" };
}

function validateOperationId(operationId: string): void {
	if (!OPERATION_ID.test(operationId)) {
		throw new Error("Storage operation ID contains unsupported characters.");
	}
}

function validatePointer(pointer: string): void {
	if (isAbsolute(pointer) || !ARTIFACT_POINTER.test(pointer)) {
		throw new Error(`Invalid database artifact pointer: ${pointer}`);
	}
}

function artifactPath(layout: StorageLayout, pointer: string): string {
	validatePointer(pointer);
	const path = resolve(layout.dbDir, pointer);
	if (!path.startsWith(`${layout.versionsDir}/`)) {
		throw new Error(`Database artifact escapes the versions directory: ${pointer}`);
	}
	return path;
}

function validateJournal(value: unknown): StorageJournal {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new Error("Storage journal must be a JSON object.");
	}
	const journal = value as Partial<StorageJournal>;
	if (journal.version !== 1) throw new Error("Unsupported storage journal version.");
	if (typeof journal.operationId !== "string")
		throw new Error("Storage journal has no operation ID.");
	validateOperationId(journal.operationId);
	if (
		journal.state !== "prepared" &&
		journal.state !== "switched" &&
		journal.state !== "committed"
	) {
		throw new Error("Storage journal has an invalid state.");
	}
	if (journal.oldPointer !== null && typeof journal.oldPointer !== "string") {
		throw new Error("Storage journal has an invalid old pointer.");
	}
	if (journal.oldPointer !== null) validatePointer(journal.oldPointer);
	if (typeof journal.newPointer !== "string")
		throw new Error("Storage journal has no new pointer.");
	validatePointer(journal.newPointer);
	if (typeof journal.artifactSha256 !== "string" || !SHA256.test(journal.artifactSha256)) {
		throw new Error("Storage journal has an invalid artifact hash.");
	}
	return journal as StorageJournal;
}

export function writeStorageJournal(layout: StorageLayout, journal: StorageJournal): void {
	ensureStorageLayout(layout);
	const validated = validateJournal(journal);
	durableReplaceFile(layout.journalPath, `${JSON.stringify(validated)}\n`);
}

function readStorageJournal(layout: StorageLayout): StorageJournal | null {
	if (!existsSync(layout.journalPath)) return null;
	let parsed: unknown;
	try {
		parsed = JSON.parse(readFileSync(layout.journalPath, "utf8"));
	} catch (error) {
		throw new Error("Storage journal is unreadable or malformed.", { cause: error });
	}
	return validateJournal(parsed);
}

export function readCurrentDatabasePointer(layout: StorageLayout): string | null {
	try {
		const info = lstatSync(layout.currentPointerPath);
		if (!info.isSymbolicLink()) throw new Error("Database current pointer is not a symbolic link.");
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
		throw error;
	}
	const pointer = readlinkSync(layout.currentPointerPath);
	validatePointer(pointer);
	return pointer;
}

export function sha256File(path: string): string {
	const hash = createHash("sha256");
	const buffer = Buffer.allocUnsafe(1024 * 1024);
	const fd = openSync(path, "r");
	try {
		for (;;) {
			const bytesRead = readSync(fd, buffer, 0, buffer.length, null);
			if (bytesRead === 0) break;
			hash.update(buffer.subarray(0, bytesRead));
		}
	} finally {
		closeSync(fd);
	}
	return hash.digest("hex");
}

function verifyArtifact(layout: StorageLayout, pointer: string, expectedSha256: string): void {
	const path = artifactPath(layout, pointer);
	if (!existsSync(path)) throw new Error(`Database artifact is missing: ${pointer}`);
	if (sha256File(path) !== expectedSha256) {
		throw new Error(`Database artifact hash mismatch: ${pointer}`);
	}
	const db = ReadOnlyActor.open(path);
	try {
		const rows = db.pragma("integrity_check") as Array<Record<string, unknown>>;
		if (rows.length !== 1 || Object.values(rows[0] ?? {})[0] !== "ok") {
			throw new Error(`SQLite integrity check failed: ${pointer}`);
		}
	} finally {
		db.close();
	}
}

function restorePointer(layout: StorageLayout, pointer: string | null): void {
	const current = readCurrentDatabasePointer(layout);
	if (pointer === null) {
		if (current === null) fsyncPath(layout.dbDir);
		else durableRemoveFile(layout.currentPointerPath);
		return;
	}
	if (!existsSync(artifactPath(layout, pointer))) {
		throw new Error(`Cannot recover missing previous database artifact: ${pointer}`);
	}
	if (current === pointer) fsyncPath(layout.dbDir);
	else durableReplaceSymlink(layout.currentPointerPath, pointer);
}

export function recoverStorageJournal(
	layout: StorageLayout,
): { action: "none" } | { action: "rolled_back" | "completed"; state: StorageJournalState } {
	ensureStorageLayout(layout);
	const journal = readStorageJournal(layout);
	if (!journal) return { action: "none" };

	const current = readCurrentDatabasePointer(layout);
	if (current !== journal.oldPointer && current !== journal.newPointer) {
		throw new Error("Storage journal does not identify the current database pointer.");
	}

	if (journal.state === "committed") {
		if (current !== journal.newPointer) {
			throw new Error("Committed storage journal does not point at the committed artifact.");
		}
		verifyArtifact(layout, journal.newPointer, journal.artifactSha256);
		durableRemoveFile(layout.journalPath);
		return { action: "completed", state: journal.state };
	}

	restorePointer(layout, journal.oldPointer);
	durableRemoveFile(layout.journalPath);
	return { action: "rolled_back", state: journal.state };
}

export function activateDatabaseArtifact(
	layout: StorageLayout,
	input: { operationId: string; pointer: string; artifactSha256: string },
): void {
	ensureStorageLayout(layout);
	validateOperationId(input.operationId);
	validatePointer(input.pointer);
	if (!SHA256.test(input.artifactSha256)) throw new Error("Invalid database artifact hash.");
	if (readStorageJournal(layout)) throw new Error("A storage journal already requires recovery.");

	const oldPointer = readCurrentDatabasePointer(layout);
	const journal: StorageJournal = {
		version: 1,
		operationId: input.operationId,
		state: "prepared",
		oldPointer,
		newPointer: input.pointer,
		artifactSha256: input.artifactSha256,
	};
	writeStorageJournal(layout, journal);
	fsyncPath(artifactPath(layout, input.pointer));
	fsyncPath(layout.versionsDir);

	try {
		durableReplaceSymlink(layout.currentPointerPath, input.pointer);
		writeStorageJournal(layout, { ...journal, state: "switched" });
		verifyArtifact(layout, input.pointer, input.artifactSha256);
		writeStorageJournal(layout, { ...journal, state: "committed" });
		durableRemoveFile(layout.journalPath);
	} catch (error) {
		try {
			if (recoverStorageJournal(layout).action === "completed") return;
		} catch (recoveryError) {
			throw new AggregateError(
				[error, recoveryError],
				"Database activation failed and storage recovery did not complete.",
			);
		}
		throw error;
	}
}

/**
 * Publish a backup already verified by the T050/T051 cutover preconditions.
 * This function is deliberately not called automatically before T051.
 */
export function runLegacyMigration(input: {
	layout: StorageLayout;
	operationId: string;
	verifiedBackupPath: string;
	verifiedBackupSha256: string;
}): void {
	ensureStorageLayout(input.layout);
	validateOperationId(input.operationId);
	if (!SHA256.test(input.verifiedBackupSha256)) throw new Error("Invalid verified backup hash.");
	if (sha256File(input.verifiedBackupPath) !== input.verifiedBackupSha256) {
		throw new Error("Verified legacy backup hash no longer matches its artifact.");
	}
	if (readCurrentDatabasePointer(input.layout) !== null) {
		throw new Error("Legacy migration refuses to replace an existing current database pointer.");
	}

	const pointer = `versions/${input.operationId}.sqlite`;
	const destination = artifactPath(input.layout, pointer);
	durableCopyFile(input.verifiedBackupPath, destination);
	activateDatabaseArtifact(input.layout, {
		operationId: input.operationId,
		pointer,
		artifactSha256: input.verifiedBackupSha256,
	});
}
