import Database from "better-sqlite3";
import { describe, expect, it } from "vitest";
import { compileDefaultCapabilityManifest } from "./capability-manifest.js";
import {
	compileProviderDestinationBoundary,
	compileRunnerLocalDestinationBoundary,
	compileUntrustedDestinationBoundary,
	destinationBoundarySql,
	isDestinationEligible,
} from "./destination-boundary.js";

const REPOSITORY_A = `repo-v1:sha256:${"a".repeat(64)}`;
const REPOSITORY_B = `repo-v1:sha256:${"b".repeat(64)}`;

function manifest(endpointUrl: string) {
	return compileDefaultCapabilityManifest({
		version: 1,
		role: "summary",
		state: "enabled",
		wireProtocol: "openai_chat_completions_v1",
		modelId: "destination-boundary-test",
		modelRevision: "1",
		endpointUrl,
		credentialRef: { kind: "none" },
	});
}

const remoteManifest = manifest("https://summary.stub.invalid/v1/chat/completions");
const localHttpManifest = manifest("http://127.0.0.1:1234/v1/chat/completions");
const localHttpsManifest = manifest("https://127.0.0.1:1234/v1/chat/completions");

const remoteProvider = compileProviderDestinationBoundary(remoteManifest, {
	repositoryIdentity: REPOSITORY_A,
	tlsPeerVerified: true,
});
const localHttpProvider = compileProviderDestinationBoundary(localHttpManifest, {
	repositoryIdentity: REPOSITORY_A,
	tlsPeerVerified: true,
});
const localHttpsUnverified = compileProviderDestinationBoundary(localHttpsManifest, {
	repositoryIdentity: REPOSITORY_A,
	tlsPeerVerified: false,
});
const localHttpsVerified = compileProviderDestinationBoundary(localHttpsManifest, {
	repositoryIdentity: REPOSITORY_A,
	tlsPeerVerified: true,
});
const remoteConsumer = compileUntrustedDestinationBoundary({
	consumer: "daemon_search",
	configurationFingerprint: remoteManifest.configurationFingerprint,
	targetAgent: "codex",
	targetModel: "gpt-5",
});
const unknownConsumer = compileUntrustedDestinationBoundary({
	consumer: "viewer",
	configurationFingerprint: remoteManifest.configurationFingerprint,
});
const runnerLocal = compileRunnerLocalDestinationBoundary({
	consumer: "hook_pack",
	configurationFingerprint: remoteManifest.configurationFingerprint,
	repositoryIdentity: REPOSITORY_A,
	targetAgent: "claude-code",
	targetModel: "runner-fixture",
});

describe("DestinationBoundaryV1", () => {
	it("derives transport trust and location instead of accepting caller claims", () => {
		expect(remoteProvider).toMatchObject({
			consumer: "summary_provider",
			executionLocation: "remote",
			repositoryIdentity: REPOSITORY_A,
			configurationFingerprint: remoteManifest.configurationFingerprint,
			providerFingerprint: remoteManifest.summaryProvider.providerFingerprint,
			providerPeerTrust: "verified",
		});
		expect(localHttpProvider).toMatchObject({
			executionLocation: "local",
			providerPeerTrust: "unverified",
		});
		expect(localHttpsUnverified).toMatchObject({
			executionLocation: "local",
			providerPeerTrust: "unverified",
		});
		expect(localHttpsVerified).toMatchObject({
			executionLocation: "local",
			providerPeerTrust: "verified",
		});
		expect(remoteConsumer).toMatchObject({
			executionLocation: "remote",
			repositoryIdentity: null,
			providerFingerprint: null,
			providerPeerTrust: "not_applicable",
		});
		expect(unknownConsumer).toMatchObject({
			executionLocation: "unknown",
			repositoryIdentity: null,
			providerFingerprint: null,
			providerPeerTrust: "not_applicable",
		});
		expect(runnerLocal).toMatchObject({
			executionLocation: "local",
			repositoryIdentity: REPOSITORY_A,
			providerFingerprint: null,
			providerPeerTrust: "not_applicable",
		});
	});

	it.each([
		["remote provider", remoteProvider, "eligible", REPOSITORY_A, true],
		["remote provider", remoteProvider, "local_only", REPOSITORY_A, false],
		["remote provider", remoteProvider, "private", REPOSITORY_A, false],
		["remote provider", remoteProvider, "secret", REPOSITORY_A, false],
		["remote consumer", remoteConsumer, "eligible", REPOSITORY_A, true],
		["remote consumer", remoteConsumer, "private", REPOSITORY_A, false],
		["unknown consumer", unknownConsumer, "eligible", REPOSITORY_A, true],
		["unknown consumer", unknownConsumer, "local_only", REPOSITORY_A, false],
		["local HTTP provider", localHttpProvider, "eligible", REPOSITORY_A, true],
		["local HTTP provider", localHttpProvider, "local_only", REPOSITORY_A, false],
		["local HTTP provider", localHttpProvider, "private", REPOSITORY_A, false],
		["local HTTPS without peer verification", localHttpsUnverified, "eligible", REPOSITORY_A, true],
		["local HTTPS without peer verification", localHttpsUnverified, "private", REPOSITORY_A, false],
		["verified local HTTPS same repository", localHttpsVerified, "eligible", REPOSITORY_A, true],
		["verified local HTTPS same repository", localHttpsVerified, "local_only", REPOSITORY_A, true],
		["verified local HTTPS same repository", localHttpsVerified, "private", REPOSITORY_A, true],
		["verified local HTTPS same repository", localHttpsVerified, "secret", REPOSITORY_A, false],
		["verified local HTTPS cross repository", localHttpsVerified, "eligible", REPOSITORY_B, true],
		[
			"verified local HTTPS cross repository",
			localHttpsVerified,
			"local_only",
			REPOSITORY_B,
			false,
		],
		["verified local HTTPS cross repository", localHttpsVerified, "private", REPOSITORY_B, false],
		["verified local HTTPS unknown repository", localHttpsVerified, "eligible", null, true],
		["verified local HTTPS unknown repository", localHttpsVerified, "local_only", null, false],
		["verified local HTTPS unknown repository", localHttpsVerified, "private", null, false],
		["runner local same repository", runnerLocal, "eligible", REPOSITORY_A, true],
		["runner local same repository", runnerLocal, "local_only", REPOSITORY_A, true],
		["runner local same repository", runnerLocal, "private", REPOSITORY_A, true],
		["runner local cross repository", runnerLocal, "private", REPOSITORY_B, false],
		["runner local unknown repository", runnerLocal, "private", null, false],
	] as const)("applies the closed matrix for %s / %s", (_label, boundary, sensitivity, repositoryIdentity, expected) => {
		expect(
			isDestinationEligible(boundary, {
				sensitivity,
				repositoryIdentity,
				captureState: "accepted",
			}),
		).toBe(expected);
	});

	it.each([
		"eligible",
		"local_only",
		"private",
		"secret",
	] as const)("denies quarantined %s rows for every destination", (sensitivity) => {
		expect(
			isDestinationEligible(localHttpsVerified, {
				sensitivity,
				repositoryIdentity: REPOSITORY_A,
				captureState: "quarantined",
			}),
		).toBe(false);
	});

	it("rejects a structurally identical plain object as an untrusted boundary", () => {
		const forged = { ...localHttpsVerified };

		expect(() =>
			isDestinationEligible(forged, {
				sensitivity: "private",
				repositoryIdentity: REPOSITORY_A,
				captureState: "accepted",
			}),
		).toThrow(/boundary|compiled|trusted/i);
		expect(() => destinationBoundarySql(forged, "candidate")).toThrow(/boundary|compiled|trusted/i);
	});

	it.each([
		[remoteProvider, "eligible", REPOSITORY_A, "accepted"],
		[remoteProvider, "private", REPOSITORY_A, "accepted"],
		[unknownConsumer, "eligible", null, "accepted"],
		[unknownConsumer, "local_only", REPOSITORY_A, "accepted"],
		[localHttpProvider, "private", REPOSITORY_A, "accepted"],
		[localHttpsVerified, "local_only", REPOSITORY_A, "accepted"],
		[localHttpsVerified, "private", REPOSITORY_B, "accepted"],
		[localHttpsVerified, "private", null, "accepted"],
		[runnerLocal, "private", REPOSITORY_A, "accepted"],
		[runnerLocal, "secret", REPOSITORY_A, "accepted"],
		[runnerLocal, "eligible", REPOSITORY_A, "quarantined"],
	] as const)("keeps SQL eligibility in parity for %s / %s / %s / %s", (boundary, sensitivity, repositoryIdentity, captureState) => {
		const db = new Database(":memory:");
		try {
			db.exec(`CREATE TABLE candidates (
					id INTEGER PRIMARY KEY,
					sensitivity TEXT NOT NULL,
					repository_identity TEXT,
					capture_state TEXT NOT NULL
				)`);
			db.prepare(
				"INSERT INTO candidates(sensitivity, repository_identity, capture_state) VALUES (?, ?, ?)",
			).run(sensitivity, repositoryIdentity, captureState);
			const predicate = destinationBoundarySql(boundary, "candidate");
			const row = db
				.prepare(
					`SELECT CASE WHEN ${predicate.clause} THEN 1 ELSE 0 END AS eligible
						 FROM candidates AS candidate WHERE id = 1`,
				)
				.get(...predicate.params) as { eligible: 0 | 1 };

			expect(Boolean(row.eligible)).toBe(
				isDestinationEligible(boundary, {
					sensitivity,
					repositoryIdentity,
					captureState,
				}),
			);
		} finally {
			db.close();
		}
	});
});
