import { describe, expect, it } from "vitest";
import { WorkerSecretScanner } from "./redaction-worker.js";
import { SecretScanner } from "./secret-scanner.js";

// Deliberately no warmRedactionWorker() call in this file: the cold-start path is what #119
// is about. outcome-evidence.test.ts and attribution-diagnostics.test.ts reach the scanner
// unwarmed too, incidentally, through recordOutcomeEvidence - that is where the flake fired.
// This pins the scenario by name so it survives if those files ever start warming. It
// anchors the scenario, not the timing: charging start-up to the scan budget only throws
// once the machine is busy enough to close the margin.
describe("WorkerSecretScanner cold start", () => {
	it("redacts on the first scan of a process, before anything warms the worker", () => {
		const scanner = new WorkerSecretScanner(new SecretScanner());
		expect(scanner.scan(`ghp_${"A".repeat(36)}`).redacted).toBe("[REDACTED:github_pat_classic]");
	});
});
