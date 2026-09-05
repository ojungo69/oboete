import { createHash } from "node:crypto";
import type { EffectiveCapabilityManifestV1 } from "./capability-manifest.js";
import type { RawEventCaptureState, SensitivityV1 } from "./types.js";

export type DestinationConsumerV1 =
	| "summary_provider"
	| "hook_pack"
	| "daemon_get"
	| "daemon_search"
	| "daemon_pack"
	| "mcp_direct"
	| "mcp_index"
	| "viewer"
	| "maintenance"
	| "export"
	| "import"
	| "dedup";

export type ProviderPeerTrustV1 = "verified" | "unverified" | "not_applicable";

export type DestinationBoundaryV1 = Readonly<{
	version: 1;
	consumer: DestinationConsumerV1;
	targetAgent: "claude-code" | "codex" | "none";
	targetModel: string | null;
	executionLocation: "local" | "remote" | "unknown";
	repositoryIdentity: string | null;
	configurationFingerprint: string;
	providerFingerprint: string | null;
	providerPeerTrust: ProviderPeerTrustV1;
}>;

type DestinationCandidate = {
	sensitivity: SensitivityV1;
	repositoryIdentity: string | null;
	captureState?: RawEventCaptureState;
};

const FINGERPRINT = /^sha256:[a-f0-9]{64}$/;
const REPOSITORY_IDENTITY = /^repo-v1:sha256:[a-f0-9]{64}$/;
const compiledBoundaries = new WeakSet<object>();

export const CAPTURE_ONLY_DESTINATION_FINGERPRINT = `sha256:${createHash("sha256")
	.update("free-mem:capture-only-destination:v1", "utf8")
	.digest("hex")}`;

function checkedFingerprint(value: string): string {
	if (!FINGERPRINT.test(value)) throw new Error("destination boundary fingerprint is invalid");
	return value;
}

function checkedRepositoryIdentity(value: string | null): string | null {
	if (value !== null && !REPOSITORY_IDENTITY.test(value)) {
		throw new Error("destination boundary repository identity is invalid");
	}
	return value;
}

function checkedModel(value: string | null | undefined): string | null {
	if (value == null) return null;
	const model = value.trim();
	if (!model || Buffer.byteLength(model, "utf8") > 256) {
		throw new Error("destination boundary model is invalid");
	}
	return model;
}

function compileBoundary(value: DestinationBoundaryV1): DestinationBoundaryV1 {
	const boundary = Object.freeze(value);
	compiledBoundaries.add(boundary);
	return boundary;
}

function assertCompiledBoundary(value: unknown): asserts value is DestinationBoundaryV1 {
	if (!value || typeof value !== "object" || !compiledBoundaries.has(value)) {
		throw new Error("destination boundary is not compiler-created");
	}
}

export function compileProviderDestinationBoundary(
	manifest: EffectiveCapabilityManifestV1,
	input: { repositoryIdentity: string | null; tlsPeerVerified: boolean },
): DestinationBoundaryV1 {
	const provider = manifest.summaryProvider;
	const peerTrust =
		provider.executionLocation === "local" && provider.tlsPolicy === "not_applicable"
			? "unverified"
			: input.tlsPeerVerified
				? "verified"
				: "unverified";
	return compileBoundary({
		version: 1,
		consumer: "summary_provider",
		targetAgent: "none",
		targetModel: checkedModel(provider.modelId),
		executionLocation: provider.executionLocation,
		repositoryIdentity: checkedRepositoryIdentity(input.repositoryIdentity),
		configurationFingerprint: checkedFingerprint(manifest.configurationFingerprint),
		providerFingerprint: checkedFingerprint(provider.providerFingerprint),
		providerPeerTrust: peerTrust,
	});
}

export function compileUntrustedDestinationBoundary(input: {
	consumer: DestinationConsumerV1;
	configurationFingerprint: string;
	targetAgent?: "claude-code" | "codex" | "none";
	targetModel?: string | null;
}): DestinationBoundaryV1 {
	return compileBoundary({
		version: 1,
		consumer: input.consumer,
		targetAgent: input.targetAgent ?? "none",
		targetModel: checkedModel(input.targetModel),
		executionLocation: input.consumer === "viewer" ? "unknown" : "remote",
		repositoryIdentity: null,
		configurationFingerprint: checkedFingerprint(input.configurationFingerprint),
		providerFingerprint: null,
		providerPeerTrust: "not_applicable",
	});
}

/** Runner-owned local fixture compiler; production RPC input never reaches this function. */
export function compileRunnerLocalDestinationBoundary(input: {
	consumer: DestinationConsumerV1;
	configurationFingerprint: string;
	repositoryIdentity: string;
	targetAgent?: "claude-code" | "codex" | "none";
	targetModel?: string | null;
}): DestinationBoundaryV1 {
	return compileBoundary({
		version: 1,
		consumer: input.consumer,
		targetAgent: input.targetAgent ?? "none",
		targetModel: checkedModel(input.targetModel),
		executionLocation: "local",
		repositoryIdentity: checkedRepositoryIdentity(input.repositoryIdentity),
		configurationFingerprint: checkedFingerprint(input.configurationFingerprint),
		providerFingerprint: null,
		providerPeerTrust: "not_applicable",
	});
}

function permitsRestricted(boundary: DestinationBoundaryV1): boolean {
	return (
		boundary.executionLocation === "local" &&
		boundary.repositoryIdentity !== null &&
		(boundary.providerPeerTrust === "verified" || boundary.providerPeerTrust === "not_applicable")
	);
}

// Allowlist, mirroring the SQL predicates below: any capture state other than
// 'accepted' and any sensitivity outside the known set is DENIED, so an enum
// addition or an off-database candidate fails closed instead of open.
export function isDestinationEligible(
	boundary: DestinationBoundaryV1,
	candidate: DestinationCandidate,
): boolean {
	assertCompiledBoundary(boundary);
	if (candidate.captureState !== undefined && candidate.captureState !== "accepted") return false;
	if (candidate.sensitivity === "eligible") return true;
	if (candidate.sensitivity !== "local_only" && candidate.sensitivity !== "private") return false;
	return (
		permitsRestricted(boundary) &&
		candidate.repositoryIdentity !== null &&
		candidate.repositoryIdentity === boundary.repositoryIdentity
	);
}

export function destinationBoundarySql(
	boundary: DestinationBoundaryV1,
	tableAlias: string,
): { clause: string; params: unknown[] } {
	assertCompiledBoundary(boundary);
	if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(tableAlias)) {
		throw new Error("destination boundary SQL alias is invalid");
	}
	const prefix = `${tableAlias}.`;
	const accepted = `${prefix}capture_state = 'accepted' AND `;
	if (!permitsRestricted(boundary)) {
		return {
			clause: `(${accepted}${prefix}sensitivity = 'eligible')`,
			params: [],
		};
	}
	return {
		clause: `(${accepted}(${prefix}sensitivity = 'eligible' OR (${prefix}sensitivity IN ('local_only', 'private') AND ${prefix}repository_identity = ?)))`,
		params: [boundary.repositoryIdentity],
	};
}

export function memoryDestinationBoundarySql(
	boundary: DestinationBoundaryV1,
	tableAlias = "memory_items",
): { clause: string; params: unknown[] } {
	assertCompiledBoundary(boundary);
	if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(tableAlias)) {
		throw new Error("destination boundary SQL alias is invalid");
	}
	const prefix = `${tableAlias}.`;
	if (!permitsRestricted(boundary)) {
		return { clause: `(${prefix}sensitivity = 'eligible')`, params: [] };
	}
	return {
		clause: `(${prefix}sensitivity = 'eligible' OR (${prefix}sensitivity IN ('local_only', 'private') AND ${prefix}repository_identity = ?))`,
		params: [boundary.repositoryIdentity],
	};
}

export function destinationBoundaryFingerprint(boundary: DestinationBoundaryV1): string {
	assertCompiledBoundary(boundary);
	// Code-unit order, never localeCompare: the fingerprint must be identical on
	// every device regardless of locale.
	const canonical = JSON.stringify(
		boundary,
		Object.keys(boundary).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)),
	);
	return `sha256:${createHash("sha256").update(`free-mem:destination-boundary:v1\0${canonical}`).digest("hex")}`;
}
