import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import * as core from "./index.js";

const dirs: string[] = [];

afterEach(() => {
	for (const dir of dirs.splice(0)) {
		rmSync(dir, { recursive: true, force: true });
	}
});

describe("Phase 1 install manifest", () => {
	it("P1-T049-01-install-manifest-roundtrip", () => {
		const dir = mkdtempSync(join(tmpdir(), "codemem-install-"));
		dirs.push(dir);
		const target = join(dir, "hooks.sh");
		const manifestPath = join(dir, "ownership.json");
		writeFileSync(target, "echo keep-me\n", { mode: 0o600 });
		core.installWithManifest({
			manifestPath,
			blocks: [
				{
					id: "hook",
					path: target,
					marker: "CODEMEM-MANAGED",
					content: "echo managed\n",
				},
			],
		});
		const installed = readFileSync(target, "utf8");
		expect(installed).toContain("echo keep-me");
		expect(installed).toContain("echo managed");
		expect(installed).toContain("# BEGIN CODEMEM-MANAGED");
		core.uninstallWithManifest(manifestPath);
		expect(readFileSync(target, "utf8")).toBe("echo keep-me\n");

		core.installWithManifest({
			manifestPath,
			blocks: [
				{
					id: "hook",
					path: target,
					marker: "CODEMEM-MANAGED",
					content: "echo managed\n",
				},
			],
		});
		writeFileSync(target, readFileSync(target, "utf8").replace("echo managed", "echo tampered"), {
			mode: 0o600,
		});
		expect(() => core.uninstallWithManifest(manifestPath)).toThrow(/fingerprint/i);
		expect(readFileSync(target, "utf8")).toContain("echo tampered");
		expect(readFileSync(target, "utf8")).toContain("echo keep-me");
	});
});
