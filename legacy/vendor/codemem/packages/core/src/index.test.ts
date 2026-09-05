import { describe, expect, it } from "vitest";
import * as core from "./index.js";

describe("core", () => {
	it("exports a version string", () => {
		expect(core.VERSION).toBe("0.40.2");
	});

	it("does not expose mutable observer config discovery", () => {
		expect(Reflect.has(core, "loadObserverConfig")).toBe(false);
	});

	it("does not expose raw capability activation writers", () => {
		for (const name of [
			"acquireDaemonWriterLease",
			"acquireCapabilityLifecycleLock",
			"activateCapabilityManifest",
			"capabilitySetupFileState",
			"probeDaemonWriterAvailable",
			"recoverCapabilitySetupTransaction",
			"writeCapabilityManifestGeneration",
			"writeCapabilitySetupJournal",
		]) {
			expect(Reflect.has(core, name), name).toBe(false);
		}
	});

	it("does not expose tiered replay construction", () => {
		expect(Reflect.has(core, "decideExtractionReplayTier")).toBe(false);
		expect(Reflect.has(core, "RICH_TIER_DEFAULTS")).toBe(false);
		expect(Reflect.has(core, "SIMPLE_TIER_DEFAULTS")).toBe(false);
	});

	it("does not expose injectable AI maintenance", () => {
		expect(Reflect.has(core, "aiBackfillStructuredContent")).toBe(false);
	});

	it("T032 keeps unrestricted provider and pack constructors out of the public barrel", () => {
		for (const name of [
			"replayBatchExtraction",
			"selectDistillCorpus",
			"buildDistillReport",
			"buildMemoryPack",
			"buildMemoryPackAsync",
			"buildMemoryPackTrace",
			"buildMemoryPackTraceAsync",
			"buildMemoryPackWithTrace",
			"buildMemoryPackWithTraceAsync",
			"estimateTokens",
		]) {
			expect(Reflect.has(core, name), name).toBe(false);
		}
	});
});
