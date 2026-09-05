import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import * as core from "./index.js";

const repoRoot = resolve(import.meta.dirname, "../../../../..");

describe("Phase 1 sole-writer boundary", () => {
	it("P1-T048-01-zero-external-db-handles", () => {
		expect(() =>
			execFileSync(
				process.execPath,
				["--experimental-strip-types", resolve(repoRoot, "harness/phase1-static-scan.ts")],
				{ encoding: "utf8", stdio: "pipe" },
			),
		).not.toThrow();
		for (const name of [
			"connect",
			"connectReadOnly",
			"ReadOnlyActor",
			"WriterActor",
			"openMigratedWriter",
			"MemoryStore",
			"openTestMemoryStore",
			"exportMemories",
			"importMemories",
			"initDatabase",
			"vacuumDatabase",
			"getMemoryArtifactReport",
			"getMemoryRoleReport",
			"getRawEventRelinkPlan",
			"getRawEventRelinkReport",
			"getRawEventStatus",
			"getReliabilityMetrics",
			"rawEventsGate",
			"retryRawEventFailures",
			"DedupKeyBackfillRunner",
			"RefBackfillRunner",
			"ScopeBackfillRunner",
			"SessionContextBackfillRunner",
			"SummaryDedupBackfillRunner",
			"VectorModelMigrationRunner",
		]) {
			expect(Reflect.get(core, name), `${name} must not be a public DB bypass`).toBeUndefined();
		}
	});
});
