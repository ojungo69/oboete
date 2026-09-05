import { createHash } from "node:crypto";
import { isIP } from "node:net";
import * as tls from "node:tls";

export const PROCESSING_JOB_CONCURRENCY = 2;
export const PROCESSING_JOB_CAPACITY = 25;
export const PROCESSING_JOB_RETRY_LIMIT = 3;
/** Slice 1 bounded job and legacy-recovery source range. */
export const PROCESSING_JOB_MAX_SOURCE_EVENTS = 100;

export type ProviderWireProtocol = "anthropic_messages_v1" | "openai_chat_completions_v1";

export type CredentialRefV1 = { kind: "none" } | { kind: "environment"; name: string };

export interface ProviderProposalV1 {
	version: 1;
	role: "summary";
	state: "enabled";
	wireProtocol: ProviderWireProtocol;
	modelId: string;
	modelRevision: string;
	endpointUrl: string;
	credentialRef: CredentialRefV1;
}

export interface ProviderChoiceV1 extends ProviderProposalV1 {
	providerFingerprint: string;
	executionLocation: "local" | "remote";
	egressPolicy: "on_device" | "explicit_remote";
	costClass: "local_zero" | "external_metered";
	tlsPolicy: "system" | "not_applicable";
	redirectPolicy: "reject";
}

export interface ResourceProfileV1 {
	profileId: "slice1-short-run";
	version: 1 | 2;
	captureConcurrencyLimit: 2;
	processingConcurrencyLimit: 2;
	processingQueueCapacity: 25;
	processingRetryLimit: 3;
	maxMemoryItemsPerDerivation: 16 | 17;
	maxSourceEventsPerJob: 100;
	observerRequestTimeoutMs: 60_000;
	observerMaxInputChars: 12_000;
	observerMaxOutputTokens: 4_000;
	observerMaxResponseBytes: 1_048_576;
	observerTemperature: 0.2;
	providerTlsPreflightTimeoutMs: 5_000;
	workerWarmLifetimeMs: 30_000;
	periodicSweepIntervalMs: 30_000;
	idleFlushMs: 120_000;
	eventDebounceMs: 1_000;
	stuckClaimTimeoutMs: 300_000;
	rawEventRetentionEnabled: false;
	rawEventRetentionMs: 0;
	resourceWarningThresholds: {
		maxSteadyProductProcessCount: 3;
		maxShortRunRssGrowthMiB: 32;
		maxPendingQueueDepth: 20;
		maxStorageGrowthBytes: 1_048_576;
	};
	injectionEnvelope: {
		selectionTimeBudgetMs: 750;
		admittedCandidateLimit: 32;
		maxRenderedBytes: 16_384;
		maxSelectedItems: 8;
		maxInjectedTokens: 800;
		laneBudgets: {
			exact_session: { minItems: 0; maxItems: 4 };
			lexical: { minItems: 0; maxItems: 8 };
			semantic: { minItems: 0; maxItems: 0 };
			recency: { minItems: 0; maxItems: 2 };
		};
	};
}

export type ProviderTransportProfileV1 = Pick<
	ResourceProfileV1,
	| "observerRequestTimeoutMs"
	| "observerMaxInputChars"
	| "observerMaxOutputTokens"
	| "observerMaxResponseBytes"
	| "observerTemperature"
>;

export type LegacyDispositionV1 = {
	key: string;
	disposition: "translated" | "ignored" | "overridden";
};

export interface EffectiveCapabilityManifestV1 {
	manifestVersion: 1;
	manifestId: string;
	baseConfigurationFingerprint?: string;
	configurationFingerprint: string;
	destinationPolicyMap: {
		"claude-code-local": {
			targetAgent: "claude-code";
			executionLocation: "local";
			egressPolicy: "same_repository_on_device";
			eligibleSensitivities: ["eligible", "private", "local_only"];
		};
		"codex-local": {
			targetAgent: "codex";
			executionLocation: "local";
			egressPolicy: "same_repository_on_device";
			eligibleSensitivities: ["eligible", "private", "local_only"];
		};
		"claude-code-remote": {
			targetAgent: "claude-code";
			executionLocation: "remote";
			egressPolicy: "remote_off_host";
			eligibleSensitivities: ["eligible"];
		};
		"codex-remote": {
			targetAgent: "codex";
			executionLocation: "remote";
			egressPolicy: "remote_off_host";
			eligibleSensitivities: ["eligible"];
		};
	};
	resourceProfile: ResourceProfileV1;
	summaryProvider: ProviderChoiceV1;
	embeddingProvider: {
		state: "disabled";
		reason: "slice1_semantic_not_owned";
		packDegradationReason: "semantic_disabled";
	};
	legacyDispositions: LegacyDispositionV1[];
}

type JsonRecord = Record<string, unknown>;

const PROVIDER_DOMAIN = "free-mem:provider-choice:v1\0";
const MANIFEST_DOMAIN = "free-mem:effective-capability-manifest:v1\0";
const FINGERPRINT = /^sha256:[0-9a-f]{64}$/;
const MANIFEST_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const LEGACY_KEY = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const ENVIRONMENT_NAME = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

const DESTINATION_POLICY_MAP: EffectiveCapabilityManifestV1["destinationPolicyMap"] = {
	"claude-code-local": {
		targetAgent: "claude-code",
		executionLocation: "local",
		egressPolicy: "same_repository_on_device",
		eligibleSensitivities: ["eligible", "private", "local_only"],
	},
	"codex-local": {
		targetAgent: "codex",
		executionLocation: "local",
		egressPolicy: "same_repository_on_device",
		eligibleSensitivities: ["eligible", "private", "local_only"],
	},
	"claude-code-remote": {
		targetAgent: "claude-code",
		executionLocation: "remote",
		egressPolicy: "remote_off_host",
		eligibleSensitivities: ["eligible"],
	},
	"codex-remote": {
		targetAgent: "codex",
		executionLocation: "remote",
		egressPolicy: "remote_off_host",
		eligibleSensitivities: ["eligible"],
	},
};

const RESOURCE_PROFILE_BASE = {
	profileId: "slice1-short-run",
	version: 1,
	captureConcurrencyLimit: 2,
	processingConcurrencyLimit: PROCESSING_JOB_CONCURRENCY,
	processingQueueCapacity: PROCESSING_JOB_CAPACITY,
	processingRetryLimit: PROCESSING_JOB_RETRY_LIMIT,
	maxMemoryItemsPerDerivation: 16,
	maxSourceEventsPerJob: PROCESSING_JOB_MAX_SOURCE_EVENTS,
	observerRequestTimeoutMs: 60_000,
	observerMaxInputChars: 12_000,
	observerMaxOutputTokens: 4_000,
	observerMaxResponseBytes: 1_048_576,
	observerTemperature: 0.2,
	providerTlsPreflightTimeoutMs: 5_000,
	workerWarmLifetimeMs: 30_000,
	periodicSweepIntervalMs: 30_000,
	idleFlushMs: 120_000,
	eventDebounceMs: 1_000,
	stuckClaimTimeoutMs: 300_000,
	rawEventRetentionEnabled: false,
	rawEventRetentionMs: 0,
	resourceWarningThresholds: {
		maxSteadyProductProcessCount: 3,
		maxShortRunRssGrowthMiB: 32,
		maxPendingQueueDepth: 20,
		maxStorageGrowthBytes: 1_048_576,
	},
	injectionEnvelope: {
		selectionTimeBudgetMs: 750,
		admittedCandidateLimit: 32,
		maxRenderedBytes: 16_384,
		maxSelectedItems: 8,
		maxInjectedTokens: 800,
		laneBudgets: {
			exact_session: { minItems: 0, maxItems: 4 },
			lexical: { minItems: 0, maxItems: 8 },
			semantic: { minItems: 0, maxItems: 0 },
			recency: { minItems: 0, maxItems: 2 },
		},
	},
} as const satisfies ResourceProfileV1;

const FROZEN_RESOURCE_PROFILE = deepFreeze(
	structuredClone(RESOURCE_PROFILE_BASE),
) as ResourceProfileV1;

const EMBEDDING_PROVIDER: EffectiveCapabilityManifestV1["embeddingProvider"] = {
	state: "disabled",
	reason: "slice1_semantic_not_owned",
	packDegradationReason: "semantic_disabled",
};

function dataRecord(value: unknown, label: string): JsonRecord {
	if (value === null || typeof value !== "object" || Array.isArray(value)) {
		throw new Error(`${label} must be an object.`);
	}
	const prototype = Object.getPrototypeOf(value);
	if (prototype !== Object.prototype && prototype !== null) {
		throw new Error(`${label} must be a plain object.`);
	}
	for (const descriptor of Object.values(Object.getOwnPropertyDescriptors(value))) {
		if (!("value" in descriptor) || !descriptor.enumerable) {
			throw new Error(`${label} must contain only enumerable data properties.`);
		}
	}
	return value as JsonRecord;
}

function exactKeys(
	value: JsonRecord,
	required: readonly string[],
	optional: readonly string[] = [],
): void {
	const allowed = new Set([...required, ...optional]);
	for (const key of Object.keys(value)) {
		if (!allowed.has(key)) throw new Error(`Unsupported field: ${key}`);
	}
	for (const key of required) {
		if (!Object.hasOwn(value, key)) throw new Error(`Missing required field: ${key}`);
	}
}

function boundedString(value: unknown, label: string, maxBytes: number, pattern?: RegExp): string {
	if (typeof value !== "string" || !value.isWellFormed()) {
		throw new Error(`${label} must be a well-formed string.`);
	}
	const bytes = Buffer.byteLength(value, "utf8");
	let hasAsciiControl = false;
	for (let index = 0; index < value.length; index++) {
		const code = value.charCodeAt(index);
		if (code <= 0x1f || code === 0x7f) {
			hasAsciiControl = true;
			break;
		}
	}
	if (bytes < 1 || bytes > maxBytes || hasAsciiControl) {
		throw new Error(`${label} is outside the closed byte/control limits.`);
	}
	if (pattern && !pattern.test(value)) throw new Error(`${label} has an invalid format.`);
	return value;
}

function assertJsonData(value: unknown, label: string): void {
	if (value === null || typeof value === "boolean") return;
	if (typeof value === "number") {
		if (!Number.isFinite(value)) throw new Error(`${label} contains a non-finite number.`);
		return;
	}
	if (typeof value === "string") {
		if (!value.isWellFormed()) throw new Error(`${label} contains malformed Unicode.`);
		return;
	}
	if (Array.isArray(value)) {
		for (const item of value) assertJsonData(item, label);
		return;
	}
	const record = dataRecord(value, label);
	for (const child of Object.values(record)) assertJsonData(child, label);
}

function canonicalJson(value: unknown): string {
	assertJsonData(value, "JCS input");
	if (
		value === null ||
		typeof value === "boolean" ||
		typeof value === "number" ||
		typeof value === "string"
	) {
		return JSON.stringify(Object.is(value, -0) ? 0 : value) as string;
	}
	if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
	const entries = Object.entries(value as JsonRecord).toSorted(([left], [right]) => {
		if (left < right) return -1;
		if (left > right) return 1;
		return 0;
	});
	const body = entries
		.map(([key, child]) => `${JSON.stringify(key)}:${canonicalJson(child)}`)
		.join(",");
	return `{${body}}`;
}

function fingerprint(domain: string, value: unknown): string {
	return `sha256:${createHash("sha256").update(domain, "utf8").update(canonicalJson(value), "utf8").digest("hex")}`;
}

function credentialRef(value: unknown): CredentialRefV1 {
	const record = dataRecord(value, "credentialRef");
	if (record.kind === "none") {
		exactKeys(record, ["kind"]);
		return { kind: "none" };
	}
	if (record.kind === "environment") {
		exactKeys(record, ["kind", "name"]);
		return {
			kind: "environment",
			name: boundedString(record.name, "credentialRef.name", 128, ENVIRONMENT_NAME),
		};
	}
	throw new Error("Unsupported credential reference.");
}

function endpointPolicy(endpointUrl: unknown, credential: CredentialRefV1) {
	if (typeof endpointUrl !== "string" || !endpointUrl.isWellFormed()) {
		throw new Error("endpointUrl must be a string.");
	}
	let ascii = true;
	for (let index = 0; index < endpointUrl.length; index++) {
		if (endpointUrl.charCodeAt(index) > 0x7f) {
			ascii = false;
			break;
		}
	}
	if (endpointUrl.length < 1 || endpointUrl.length > 2_048 || !ascii) {
		throw new Error("endpointUrl must be 1-2048 ASCII bytes.");
	}
	let endpoint: URL;
	try {
		endpoint = new URL(endpointUrl);
	} catch (error) {
		throw new Error("endpointUrl is invalid.", { cause: error });
	}
	if (endpoint.href !== endpointUrl) throw new Error("endpointUrl is not canonical.");
	if (
		(endpoint.protocol !== "http:" && endpoint.protocol !== "https:") ||
		!endpoint.hostname ||
		endpoint.pathname === "/" ||
		endpoint.username ||
		endpoint.password ||
		endpoint.search ||
		endpoint.hash
	) {
		throw new Error("endpointUrl is not a complete closed request URL.");
	}

	const hostname = endpoint.hostname;
	const bareHostname = hostname.startsWith("[") ? hostname.slice(1, -1) : hostname;
	const local = hostname === "127.0.0.1" || hostname === "[::1]";
	const mapped = /^\[::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})\]$/i.exec(hostname);
	const mappedIpv4 = mapped
		? Number.parseInt(mapped[1] as string, 16) * 65_536 + Number.parseInt(mapped[2] as string, 16)
		: null;
	const rejected =
		hostname.endsWith(".") ||
		hostname === "localhost" ||
		hostname.endsWith(".localhost") ||
		hostname === "0.0.0.0" ||
		hostname === "[::]" ||
		mappedIpv4 === 0 ||
		(mappedIpv4 !== null && Math.floor(mappedIpv4 / 0x1000000) === 127) ||
		(isIP(bareHostname) === 4 && hostname.startsWith("127.") && !local);
	if (rejected) throw new Error("endpointUrl uses an unsupported local or wildcard host.");
	if (!local && endpoint.protocol !== "https:") throw new Error("Remote providers require HTTPS.");
	if (local && endpoint.protocol === "http:" && credential.kind !== "none") {
		throw new Error("Local HTTP providers cannot use credentials.");
	}
	return {
		executionLocation: local ? ("local" as const) : ("remote" as const),
		egressPolicy: local ? ("on_device" as const) : ("explicit_remote" as const),
		costClass: local ? ("local_zero" as const) : ("external_metered" as const),
		tlsPolicy:
			local && endpoint.protocol === "http:" ? ("not_applicable" as const) : ("system" as const),
		redirectPolicy: "reject" as const,
	};
}

export function compileProviderChoice(input: unknown): ProviderChoiceV1 {
	const proposal = dataRecord(input, "ProviderProposalV1");
	exactKeys(proposal, [
		"version",
		"role",
		"state",
		"wireProtocol",
		"modelId",
		"modelRevision",
		"endpointUrl",
		"credentialRef",
	]);
	if (proposal.version !== 1 || proposal.role !== "summary" || proposal.state !== "enabled") {
		throw new Error("Provider proposal has an unsupported version, role, or state.");
	}
	const wireProtocol = proposal.wireProtocol;
	if (wireProtocol !== "anthropic_messages_v1" && wireProtocol !== "openai_chat_completions_v1") {
		throw new Error("Unsupported provider wire protocol.");
	}
	const credential = credentialRef(proposal.credentialRef);
	const choiceWithoutFingerprint: Omit<ProviderChoiceV1, "providerFingerprint"> = {
		version: 1 as const,
		role: "summary" as const,
		state: "enabled" as const,
		wireProtocol,
		modelId: boundedString(proposal.modelId, "modelId", 256),
		modelRevision: boundedString(proposal.modelRevision, "modelRevision", 128),
		endpointUrl: proposal.endpointUrl as string,
		credentialRef: credential,
		...endpointPolicy(proposal.endpointUrl, credential),
	};
	return deepFreeze({
		...choiceWithoutFingerprint,
		providerFingerprint: fingerprint(PROVIDER_DOMAIN, choiceWithoutFingerprint),
	});
}

export function validateProviderChoice(value: unknown): ProviderChoiceV1 {
	const choice = dataRecord(value, "ProviderChoiceV1");
	exactKeys(choice, [
		"version",
		"role",
		"state",
		"wireProtocol",
		"modelId",
		"modelRevision",
		"endpointUrl",
		"credentialRef",
		"providerFingerprint",
		"executionLocation",
		"egressPolicy",
		"costClass",
		"tlsPolicy",
		"redirectPolicy",
	]);
	const compiled = compileProviderChoice(providerProposal(choice));
	if (canonicalJson(compiled) !== canonicalJson(choice)) {
		throw new Error("Provider choice fingerprint or derived policy mismatch.");
	}
	return compiled;
}

export function validateProviderTransportProfile(value: unknown): ProviderTransportProfileV1 {
	const expected: ProviderTransportProfileV1 = {
		observerRequestTimeoutMs: RESOURCE_PROFILE_BASE.observerRequestTimeoutMs,
		observerMaxInputChars: RESOURCE_PROFILE_BASE.observerMaxInputChars,
		observerMaxOutputTokens: RESOURCE_PROFILE_BASE.observerMaxOutputTokens,
		observerMaxResponseBytes: RESOURCE_PROFILE_BASE.observerMaxResponseBytes,
		observerTemperature: RESOURCE_PROFILE_BASE.observerTemperature,
	};
	return fixedValue(value, expected, "provider transport profile");
}

export function providerTransportProfile(profile: ResourceProfileV1): ProviderTransportProfileV1 {
	return {
		observerRequestTimeoutMs: profile.observerRequestTimeoutMs,
		observerMaxInputChars: profile.observerMaxInputChars,
		observerMaxOutputTokens: profile.observerMaxOutputTokens,
		observerMaxResponseBytes: profile.observerMaxResponseBytes,
		observerTemperature: profile.observerTemperature,
	};
}

function fixedValue<T>(value: unknown, expected: T, label: string): T {
	assertJsonData(value, label);
	if (canonicalJson(value) !== canonicalJson(expected))
		throw new Error(`${label} is not the closed Slice 1 value.`);
	return structuredClone(expected);
}

function resourceProfile(value: unknown): ResourceProfileV1 {
	const record = dataRecord(value, "resourceProfile");
	const expected =
		record.version === 2
			? { ...RESOURCE_PROFILE_BASE, version: 2 as const, maxMemoryItemsPerDerivation: 17 as const }
			: RESOURCE_PROFILE_BASE;
	return fixedValue(value, expected, "resourceProfile") as ResourceProfileV1;
}

function legacyDispositions(value: unknown): LegacyDispositionV1[] {
	if (!Array.isArray(value) || value.length > 64) {
		throw new Error("legacyDispositions must contain at most 64 entries.");
	}
	const seen = new Set<string>();
	return value.map((entry) => {
		const record = dataRecord(entry, "legacy disposition");
		exactKeys(record, ["key", "disposition"]);
		const key = boundedString(record.key, "legacy disposition key", 128, LEGACY_KEY);
		if (seen.has(key)) throw new Error("legacyDispositions contains a duplicate key.");
		seen.add(key);
		if (
			record.disposition !== "translated" &&
			record.disposition !== "ignored" &&
			record.disposition !== "overridden"
		) {
			throw new Error("Unsupported legacy disposition.");
		}
		return { key, disposition: record.disposition };
	});
}

export function compileCapabilityManifest(input: unknown): EffectiveCapabilityManifestV1 {
	const proposal = dataRecord(input, "EffectiveCapabilityManifestV1 input");
	exactKeys(
		proposal,
		[
			"manifestVersion",
			"manifestId",
			"destinationPolicyMap",
			"resourceProfile",
			"summaryProvider",
			"embeddingProvider",
			"legacyDispositions",
		],
		["baseConfigurationFingerprint"],
	);
	if (proposal.manifestVersion !== 1) throw new Error("Unsupported manifest version.");
	const manifestId = boundedString(proposal.manifestId, "manifestId", 128, MANIFEST_ID);
	let baseConfigurationFingerprint: string | undefined;
	if (Object.hasOwn(proposal, "baseConfigurationFingerprint")) {
		if (
			typeof proposal.baseConfigurationFingerprint !== "string" ||
			!FINGERPRINT.test(proposal.baseConfigurationFingerprint)
		) {
			throw new Error("Invalid base configuration fingerprint.");
		}
		baseConfigurationFingerprint = proposal.baseConfigurationFingerprint;
	}
	const profile = resourceProfile(proposal.resourceProfile);
	if (profile.version === 2 && !baseConfigurationFingerprint) {
		throw new Error("The output-limit successor requires a base configuration fingerprint.");
	}
	const withoutFingerprint = {
		manifestVersion: 1 as const,
		manifestId,
		...(baseConfigurationFingerprint ? { baseConfigurationFingerprint } : {}),
		destinationPolicyMap: fixedValue(
			proposal.destinationPolicyMap,
			DESTINATION_POLICY_MAP,
			"destinationPolicyMap",
		),
		resourceProfile: profile,
		summaryProvider: compileProviderChoice(proposal.summaryProvider),
		embeddingProvider: fixedValue(
			proposal.embeddingProvider,
			EMBEDDING_PROVIDER,
			"embeddingProvider",
		),
		legacyDispositions: legacyDispositions(proposal.legacyDispositions),
	};
	return deepFreeze({
		...withoutFingerprint,
		configurationFingerprint: fingerprint(MANIFEST_DOMAIN, withoutFingerprint),
	});
}

export function compileDefaultCapabilityManifest(
	summaryProvider: unknown,
	legacyDispositions: LegacyDispositionV1[] = [],
	baseConfigurationFingerprint?: string,
): EffectiveCapabilityManifestV1 {
	return compileCapabilityManifest({
		manifestVersion: 1,
		manifestId: "slice1-effective-manifest-v1",
		...(baseConfigurationFingerprint ? { baseConfigurationFingerprint } : {}),
		destinationPolicyMap: DESTINATION_POLICY_MAP,
		resourceProfile: RESOURCE_PROFILE_BASE,
		summaryProvider,
		embeddingProvider: EMBEDDING_PROVIDER,
		legacyDispositions,
	});
}

export function defaultResourceProfile(): ResourceProfileV1 {
	return FROZEN_RESOURCE_PROFILE;
}

export function safeManifestProjection(
	manifest: EffectiveCapabilityManifestV1,
	providerHealth: "available" | "provider_unavailable" | "provider_tls_rejected" = "available",
	activationReceipt: "absent" | "validated" | "rejected" = "absent",
	schemaReadiness: "pending_schema_v21" | "ready" = "pending_schema_v21",
) {
	const validated = validateCapabilityManifest(manifest);
	const ready =
		schemaReadiness === "ready" &&
		activationReceipt === "validated" &&
		providerHealth === "available";
	const runtimeReason =
		schemaReadiness !== "ready"
			? ("pending_schema_v21" as const)
			: providerHealth !== "available"
				? providerHealth
				: activationReceipt !== "validated"
					? ("pending_privacy_boundary" as const)
					: ("ready" as const);
	return deepFreeze({
		mode: "configured" as const,
		configurationFingerprint: validated.configurationFingerprint,
		manifestId: validated.manifestId,
		summaryProvider: validated.summaryProvider,
		embeddingProvider: validated.embeddingProvider,
		resourceProfile: validated.resourceProfile,
		runtimeReason,
		providerHealth,
		activationReceipt,
		providerEnabled: ready,
		sweeperEnabled: ready,
		lexicalEnabled: true,
		schemaReadiness,
		packReadiness: ready ? ("ready" as const) : ("pending_pack_boundary" as const),
	});
}

export function captureOnlyCapabilityProjection(
	schemaReadiness: "pending_schema_v21" | "ready" = "pending_schema_v21",
) {
	return deepFreeze({
		mode: "capture_only" as const,
		configurationFingerprint: null,
		providerFingerprint: null,
		runtimeReason: "manifest_absent" as const,
		providerHealth: "not_configured" as const,
		providerEnabled: false,
		sweeperEnabled: false,
		lexicalEnabled: true,
		schemaReadiness,
		packReadiness: "pending_pack_boundary" as const,
	});
}

function providerProposal(choice: JsonRecord): JsonRecord {
	const {
		providerFingerprint: _providerFingerprint,
		executionLocation: _executionLocation,
		egressPolicy: _egressPolicy,
		costClass: _costClass,
		tlsPolicy: _tlsPolicy,
		redirectPolicy: _redirectPolicy,
		...proposal
	} = choice;
	return proposal;
}

export function validateCapabilityManifest(value: unknown): EffectiveCapabilityManifestV1 {
	const manifest = dataRecord(value, "EffectiveCapabilityManifestV1");
	exactKeys(
		manifest,
		[
			"manifestVersion",
			"manifestId",
			"configurationFingerprint",
			"destinationPolicyMap",
			"resourceProfile",
			"summaryProvider",
			"embeddingProvider",
			"legacyDispositions",
		],
		["baseConfigurationFingerprint"],
	);
	if (
		typeof manifest.configurationFingerprint !== "string" ||
		!FINGERPRINT.test(manifest.configurationFingerprint)
	) {
		throw new Error("Invalid configuration fingerprint.");
	}
	const choice = dataRecord(manifest.summaryProvider, "ProviderChoiceV1");
	const {
		configurationFingerprint: storedConfigurationFingerprint,
		summaryProvider: _summaryProvider,
		...manifestInput
	} = manifest;
	const compiled = compileCapabilityManifest({
		...manifestInput,
		summaryProvider: providerProposal(choice),
	});
	if (compiled.configurationFingerprint !== storedConfigurationFingerprint) {
		throw new Error("Capability manifest fingerprint mismatch.");
	}
	if (canonicalJson(compiled.summaryProvider) !== canonicalJson(choice)) {
		throw new Error("Provider choice fingerprint or derived policy mismatch.");
	}
	return compiled;
}

function deepFreeze<T>(value: T): T {
	if (value && typeof value === "object" && !Object.isFrozen(value)) {
		for (const child of Object.values(value as JsonRecord)) deepFreeze(child);
		Object.freeze(value);
	}
	return value;
}

export interface TlsPreflightConnectorInput {
	host: string;
	port: number;
	servername: string | null;
	timeoutMs: number;
	rejectUnauthorized: true;
}

export type TlsPreflightConnector = (input: TlsPreflightConnectorInput) => Promise<{
	chainVerified: boolean;
	hostnameVerified: boolean;
	peerCertificateSha256: string | null;
}>;

export class ProviderTlsPreflightError extends Error {
	constructor(
		readonly reason: "provider_unavailable" | "provider_tls_rejected",
		message: string,
		options?: ErrorOptions,
	) {
		super(message, options);
		this.name = "ProviderTlsPreflightError";
	}
}

async function nativeTlsPreflight(input: TlsPreflightConnectorInput) {
	return await new Promise<{
		chainVerified: boolean;
		hostnameVerified: boolean;
		peerCertificateSha256: string | null;
	}>((resolvePreflight, rejectPreflight) => {
		// biome-ignore lint: Keep the TLS dialer outside the SQLite-handle classifier.
		const socket = tls["connect"]({
			host: input.host,
			port: input.port,
			...(input.servername ? { servername: input.servername } : {}),
			rejectUnauthorized: true,
		});
		let settled = false;
		let absoluteDeadline: ReturnType<typeof setTimeout> | undefined;
		const finish = (error?: Error) => {
			if (settled) return;
			settled = true;
			if (absoluteDeadline !== undefined) clearTimeout(absoluteDeadline);
			socket.destroy();
			if (error) rejectPreflight(error);
		};
		absoluteDeadline = setTimeout(
			() =>
				finish(
					new ProviderTlsPreflightError("provider_unavailable", "Provider preflight timed out."),
				),
			input.timeoutMs,
		);
		socket.setTimeout(input.timeoutMs);
		socket.once("timeout", () =>
			finish(
				new ProviderTlsPreflightError("provider_unavailable", "Provider preflight timed out."),
			),
		);
		socket.once("error", (error) => {
			const code = (error as NodeJS.ErrnoException).code ?? "";
			const rejected =
				/cert|self.signed|unable.to.verify|altname|hostname/i.test(code) ||
				(!code && /certificate|self.signed|unable to verify|altname|hostname/i.test(error.message));
			finish(
				new ProviderTlsPreflightError(
					rejected ? "provider_tls_rejected" : "provider_unavailable",
					rejected ? "Provider TLS peer verification failed." : "Provider is unavailable.",
					{ cause: error },
				),
			);
		});
		socket.once("secureConnect", () => {
			if (settled) return;
			const certificate = socket.getPeerCertificate(true);
			const raw = certificate.raw;
			if (!socket.authorized || !raw) {
				finish(
					new ProviderTlsPreflightError(
						"provider_tls_rejected",
						"Provider TLS preflight rejected the peer.",
					),
				);
				return;
			}
			finish();
			resolvePreflight({
				chainVerified: true,
				hostnameVerified: true,
				peerCertificateSha256: `sha256:${createHash("sha256").update(raw).digest("hex")}`,
			});
		});
	});
}

export async function preflightProviderTls(
	provider: unknown,
	options: {
		connect?: TlsPreflightConnector;
		environment?: Record<string, string | undefined>;
	} = {},
): Promise<{ peerCertificateSha256: string } | { skipped: "local_http" }> {
	const compiled = validateProviderChoice(provider);
	if (compiled.tlsPolicy === "not_applicable") return { skipped: "local_http" };

	const environment = options.environment ?? process.env;
	if (
		environment.NODE_TLS_REJECT_UNAUTHORIZED === "0" ||
		environment.NODE_EXTRA_CA_CERTS?.trim() ||
		environment.SSL_CERT_FILE?.trim() ||
		environment.SSL_CERT_DIR?.trim()
	) {
		throw new ProviderTlsPreflightError(
			"provider_tls_rejected",
			"Provider TLS preflight rejected a trust override.",
		);
	}
	const endpoint = new URL(compiled.endpointUrl);
	const bareHostname = endpoint.hostname.startsWith("[")
		? endpoint.hostname.slice(1, -1)
		: endpoint.hostname;
	let result: Awaited<ReturnType<TlsPreflightConnector>>;
	try {
		result = await (options.connect ?? nativeTlsPreflight)({
			host: bareHostname,
			port: endpoint.port ? Number(endpoint.port) : 443,
			servername: isIP(bareHostname) === 0 ? bareHostname : null,
			timeoutMs: RESOURCE_PROFILE_BASE.providerTlsPreflightTimeoutMs,
			rejectUnauthorized: true,
		});
	} catch (error) {
		if (error instanceof ProviderTlsPreflightError) throw error;
		throw new ProviderTlsPreflightError("provider_unavailable", "Provider is unavailable.", {
			cause: error,
		});
	}
	if (
		!result.chainVerified ||
		!result.hostnameVerified ||
		!result.peerCertificateSha256 ||
		!FINGERPRINT.test(result.peerCertificateSha256)
	) {
		throw new ProviderTlsPreflightError(
			"provider_tls_rejected",
			"Provider TLS preflight rejected the peer.",
		);
	}
	return { peerCertificateSha256: result.peerCertificateSha256 };
}
