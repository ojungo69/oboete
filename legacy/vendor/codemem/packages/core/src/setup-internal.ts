import { join } from "node:path";
import type { EffectiveCapabilityManifestV1 } from "./capability-manifest.js";
import { preflightProviderTls } from "./capability-manifest.js";
import {
	acquireDaemonWriterLease,
	probeDaemonWriterAvailable,
	readDaemonHealth,
} from "./daemon-lifecycle.js";
import { readCodememConfigFileWithStatus } from "./observer-config.js";
import { acquireSpoolLock } from "./spool.js";
import {
	acquireCapabilityLifecycleLock,
	assertCapabilityActivationBase,
	type CapabilitySetupJournal,
	readCurrentCapabilityManifest,
	readValidatedCapabilityActivationReceipt,
	recoverCapabilitySetupTransaction,
	resolveStorageLayout,
	writeCapabilitySetupJournal,
} from "./storage.js";

export function readLegacyCapabilityConfigForSetup(): {
	config: Record<string, unknown>;
	degraded: boolean;
} {
	return readCodememConfigFileWithStatus();
}

export function withCapabilityLaneSetupTransaction(input: {
	dataDir: string;
	run: () => boolean;
}): boolean {
	const layout = resolveStorageLayout(input.dataDir);
	const lifecycle = acquireCapabilityLifecycleLock(layout);
	let setupLock: ReturnType<typeof acquireSpoolLock> | null = null;
	let writerLease: ReturnType<typeof acquireDaemonWriterLease> | null = null;
	try {
		if (readDaemonHealth(input.dataDir).status !== "not_running") {
			throw new Error("A daemon is running; lane-only setup stopped before mutation.");
		}
		setupLock = acquireSpoolLock(input.dataDir);
		if (!probeDaemonWriterAvailable(input.dataDir)) {
			throw new Error("A daemon is running; lane-only setup stopped before mutation.");
		}
		if (recoverCapabilitySetupTransaction(layout, lifecycle).action !== "none") {
			throw new Error(
				"Recovered an interrupted setup; re-run plain `codemem setup` before lane-only setup.",
			);
		}
		writerLease = acquireDaemonWriterLease(input.dataDir);
		return input.run();
	} finally {
		try {
			writerLease?.close();
		} finally {
			try {
				setupLock?.close();
			} finally {
				lifecycle.close();
			}
		}
	}
}

export async function withCapabilitySetupTransaction<T>(input: {
	dataDir: string;
	manifest: EffectiveCapabilityManifestV1;
	expectedCurrentFingerprint: string | null;
	run: (transaction: {
		writeJournal(journal: CapabilitySetupJournal): void;
		recover(): void;
	}) => Promise<T> | T;
}): Promise<T> {
	const layout = resolveStorageLayout(input.dataDir);
	const lifecycle = acquireCapabilityLifecycleLock(layout);
	let setupLock: ReturnType<typeof acquireSpoolLock> | null = null;
	let writerLease: ReturnType<typeof acquireDaemonWriterLease> | null = null;
	try {
		if (readDaemonHealth(input.dataDir).status !== "not_running") {
			throw new Error("A daemon is running; setup stopped before mutation.");
		}
		const assertPrestate = () => {
			const current = readCurrentCapabilityManifest(layout);
			if ((current?.configurationFingerprint ?? null) !== input.expectedCurrentFingerprint) {
				throw new Error("Capability current changed after disclosure; re-run setup.");
			}
			assertCapabilityActivationBase(layout, input.manifest);
		};
		setupLock = acquireSpoolLock(input.dataDir);
		if (!probeDaemonWriterAvailable(input.dataDir)) {
			throw new Error("A daemon is running; setup stopped before mutation.");
		}
		if (recoverCapabilitySetupTransaction(layout, lifecycle).action !== "none") {
			throw new Error(
				"Recovered an interrupted setup; re-run setup to activate the disclosed manifest.",
			);
		}
		assertPrestate();
		setupLock.close();
		setupLock = null;
		await preflightProviderTls(input.manifest.summaryProvider);
		setupLock = acquireSpoolLock(input.dataDir);
		writerLease = acquireDaemonWriterLease(input.dataDir);
		assertPrestate();
		let journalWritten = false;
		let finalized = false;
		const result = await input.run({
			writeJournal(journal): void {
				if (journalWritten) throw new Error("Capability setup journal was already written.");
				if (journal.configurationFingerprint !== input.manifest.configurationFingerprint) {
					throw new Error("Capability setup journal does not match the disclosed manifest.");
				}
				const requiredTargets = new Set([
					join(layout.capabilityManifestsDir, `${input.manifest.configurationFingerprint}.json`),
					layout.capabilityCurrentPointerPath,
					layout.capabilityActivationReceiptPath,
					layout.installManifestPath,
				]);
				for (const target of journal.targets) requiredTargets.delete(target.path);
				if (requiredTargets.size > 0) {
					throw new Error("Capability setup journal is missing a required publication target.");
				}
				writeCapabilitySetupJournal(layout, journal);
				journalWritten = true;
			},
			recover(): void {
				if (!journalWritten) throw new Error("Capability setup journal was not written.");
				finalized ||= recoverCapabilitySetupTransaction(layout, lifecycle).action === "completed";
			},
		});
		if (!journalWritten) throw new Error("Capability setup journal was not written.");
		if (!finalized) throw new Error("Capability setup transaction was not finalized.");
		const active = readCurrentCapabilityManifest(layout);
		if (active?.configurationFingerprint !== input.manifest.configurationFingerprint) {
			throw new Error("Capability setup did not publish the disclosed manifest.");
		}
		if (!readValidatedCapabilityActivationReceipt(layout, active)) {
			throw new Error("Capability setup did not publish a valid activation receipt.");
		}
		return result;
	} finally {
		try {
			writerLease?.close();
		} finally {
			setupLock?.close();
			lifecycle.close();
		}
	}
}
