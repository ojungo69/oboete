import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { ProviderTlsPreflightError } from "./capability-manifest.js";
import {
	compileProviderDestinationBoundary,
	isDestinationEligible,
} from "./destination-boundary.js";
import * as core from "./index.js";

const nativeTlsConnect = vi.hoisted(() => ({
	run: undefined as ((...args: unknown[]) => unknown) | undefined,
}));

vi.mock("node:tls", async (importOriginal) => {
	const actual = await importOriginal<typeof import("node:tls")>();
	return {
		...actual,
		connect: ((...args: unknown[]) =>
			nativeTlsConnect.run
				? nativeTlsConnect.run(...args)
				: Reflect.apply(actual.connect, actual, args)) as typeof actual.connect,
	};
});

type JsonObject = Record<string, unknown>;

type CompileProviderChoice = (proposal: unknown) => JsonObject;
type CompileCapabilityManifest = (input: unknown) => JsonObject;
type TlsPreflightConnector = (input: {
	host: string;
	port: number;
	servername: string | null;
	timeoutMs: number;
	rejectUnauthorized: true;
}) => Promise<{
	chainVerified: boolean;
	hostnameVerified: boolean;
	peerCertificateSha256: string | null;
}>;
type PreflightProviderTls = (
	provider: JsonObject,
	options?: {
		connect?: TlsPreflightConnector;
		environment?: Record<string, string | undefined>;
	},
) => Promise<{ peerCertificateSha256: string } | { skipped: "local_http" }>;

const fixturePath = fileURLToPath(
	new URL(
		"../../../../../specs/005-product-reset/fixtures/slice1-bidirectional-en-v1.json",
		import.meta.url,
	),
);
const fixture = JSON.parse(readFileSync(fixturePath, "utf8")) as {
	effectiveConfiguration: JsonObject;
	localDerivationManifest: JsonObject;
	repairedRemoteManifest: JsonObject;
	outputLimitRecoveryManifest: JsonObject;
};

function publicFunction<T>(name: string): T {
	const value = Reflect.get(core, name);
	expect(value, `${name} must be exported by @codemem/core`).toBeTypeOf("function");
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
		...input
	} = manifest;
	return { ...input, summaryProvider: providerProposal(summaryProvider as JsonObject) };
}

describe("Slice 1 capability manifest compiler", () => {
	it("compiles the committed remote proposal to the exact closed provider choice", () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const expected = fixture.effectiveConfiguration.summaryProvider as JsonObject;

		expect(compileProviderChoice(providerProposal(expected))).toEqual(expected);
	});

	it("deep-freezes the compiled provider choice", () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const expected = fixture.effectiveConfiguration.summaryProvider as JsonObject;
		const choice = compileProviderChoice(providerProposal(expected));

		expect(Object.isFrozen(choice)).toBe(true);
		expect(Object.isFrozen(choice.credentialRef)).toBe(true);
	});

	it.each([
		["http://127.0.0.1:1234/v1/chat/completions", "not_applicable"],
		["https://127.0.0.1:1234/v1/chat/completions", "system"],
		["http://[::1]:1234/v1/chat/completions", "not_applicable"],
		["https://[::1]:1234/v1/chat/completions", "system"],
	])("accepts only the exact local literal %s", (endpointUrl, tlsPolicy) => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const local = providerProposal(fixture.localDerivationManifest.summaryProvider as JsonObject);
		const choice = compileProviderChoice({
			...local,
			endpointUrl,
			credentialRef: { kind: "none" },
		});

		expect(choice).toMatchObject({
			endpointUrl,
			executionLocation: "local",
			egressPolicy: "on_device",
			costClass: "local_zero",
			tlsPolicy,
			redirectPolicy: "reject",
		});
		expect(choice.providerFingerprint).toMatch(/^sha256:[a-f0-9]{64}$/);
	});

	it.each([
		"http://localhost:1234/v1/chat/completions",
		"http://api.localhost:1234/v1/chat/completions",
		"http://localhost.:1234/v1/chat/completions",
		"https://summary.stub.invalid./v1/chat/completions",
		"http://0.0.0.0:1234/v1/chat/completions",
		"http://127.0.0.2:1234/v1/chat/completions",
		"http://127.1:1234/v1/chat/completions",
		"http://2130706433:1234/v1/chat/completions",
		"http://[::]:1234/v1/chat/completions",
		"http://[::ffff:0.0.0.0]:1234/v1/chat/completions",
		"http://[::ffff:127.0.0.1]:1234/v1/chat/completions",
		"http://[::ffff:0:0]:1234/v1/chat/completions",
		"http://[::ffff:7f00:1]:1234/v1/chat/completions",
		"http://summary.stub.invalid/v1/chat/completions",
	])("rejects ambiguous, wildcard, alternate-loopback, or remote-HTTP endpoint %s", (endpointUrl) => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const proposal = providerProposal(
			fixture.localDerivationManifest.summaryProvider as JsonObject,
		);

		expect(() => compileProviderChoice({ ...proposal, endpointUrl })).toThrow();
	});

	it("classifies the fixed runner hostname as remote without resolving DNS", () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const expected = fixture.effectiveConfiguration.summaryProvider as JsonObject;

		expect(compileProviderChoice(providerProposal(expected))).toMatchObject({
			endpointUrl: "https://summary.stub.invalid/v1/chat/completions",
			executionLocation: "remote",
			egressPolicy: "explicit_remote",
			costClass: "external_metered",
			tlsPolicy: "system",
			redirectPolicy: "reject",
		});
	});

	it("classifies a canonical non-local IPv4-mapped IPv6 endpoint as remote", () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const proposal = providerProposal(fixture.effectiveConfiguration.summaryProvider as JsonObject);
		const endpointUrl = "https://[::ffff:c000:201]/v1/chat/completions";

		expect(compileProviderChoice({ ...proposal, endpointUrl })).toMatchObject({
			endpointUrl,
			executionLocation: "remote",
			egressPolicy: "explicit_remote",
			costClass: "external_metered",
			tlsPolicy: "system",
		});
	});

	it.each([
		["inline credential", { credentialRef: { kind: "inline", value: "not-a-real-secret" } }],
		["invalid environment name", { credentialRef: { kind: "environment", name: "1INVALID" } }],
		["root-only endpoint", { endpointUrl: "https://summary.stub.invalid/" }],
		[
			"query-bearing endpoint",
			{ endpointUrl: "https://summary.stub.invalid/v1/chat/completions?debug=1" },
		],
		[
			"username-bearing endpoint",
			{ endpointUrl: "https://user@summary.stub.invalid/v1/chat/completions" },
		],
		["unsupported protocol", { wireProtocol: "openai_responses_v1" }],
		["control-bearing model", { modelId: "model\nname" }],
		["caller UID authority", { trustedUid: 1000 }],
		["caller CA path", { caPath: "/synthetic/ca.pem" }],
		["caller TLS disable", Object.fromEntries([["rejectUnauthorized", false]])],
		["self-declared fingerprint", { providerFingerprint: "sha256:self-declared" }],
	])("rejects closed-shape or trust override: %s", (_label, mutation) => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const proposal = providerProposal(fixture.effectiveConfiguration.summaryProvider as JsonObject);

		expect(() => compileProviderChoice({ ...proposal, ...mutation })).toThrow();
	});

	it("requires credential-none for local HTTP", () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const local = providerProposal(fixture.localDerivationManifest.summaryProvider as JsonObject);

		expect(() =>
			compileProviderChoice({
				...local,
				endpointUrl: "http://127.0.0.1:1234/v1/chat/completions",
				credentialRef: { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" },
			}),
		).toThrow();
	});

	it("recomputes the committed JCS provider and manifest fingerprints", () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const compileCapabilityManifest = publicFunction<CompileCapabilityManifest>(
			"compileCapabilityManifest",
		);
		const expected = fixture.effectiveConfiguration;

		expect(
			compileProviderChoice(providerProposal(expected.summaryProvider as JsonObject))
				.providerFingerprint,
		).toBe("sha256:d184deae938722877e017d85ab382a4f72c287857bf0f346f483263680635ede");
		expect(compileCapabilityManifest(manifestProposal(expected))).toEqual(expected);
		expect(compileCapabilityManifest(manifestProposal(expected)).configurationFingerprint).toBe(
			"sha256:2a5a5d2d3803d8f2dc2767981cbbf4f77cffc3aae8cebdc9d310e7645b27d53d",
		);
	});

	it("accepts the bound v1 successors and only the closed v2 output-limit successor", () => {
		const compileCapabilityManifest = publicFunction<CompileCapabilityManifest>(
			"compileCapabilityManifest",
		);

		for (const expected of [
			fixture.localDerivationManifest,
			fixture.repairedRemoteManifest,
			fixture.outputLimitRecoveryManifest,
		]) {
			expect(compileCapabilityManifest(manifestProposal(expected))).toEqual(expected);
		}

		const output = manifestProposal(fixture.outputLimitRecoveryManifest);
		const profile = output.resourceProfile as JsonObject;
		for (const resourceProfile of [
			{ ...profile, version: 1, maxMemoryItemsPerDerivation: 17 },
			{ ...profile, version: 2, maxMemoryItemsPerDerivation: 16 },
			{ ...profile, version: 2, maxMemoryItemsPerDerivation: 17, idleFlushMs: 1 },
			{ ...profile, profileId: "custom" },
		]) {
			expect(() => compileCapabilityManifest({ ...output, resourceProfile })).toThrow();
		}
	});

	it("rejects unknown or self-declared fields on the manifest input", () => {
		const compileCapabilityManifest = publicFunction<CompileCapabilityManifest>(
			"compileCapabilityManifest",
		);
		const input = manifestProposal(fixture.effectiveConfiguration);

		expect(() => compileCapabilityManifest({ ...input, unknown: true })).toThrow();
		expect(() =>
			compileCapabilityManifest({
				...input,
				configurationFingerprint: fixture.effectiveConfiguration.configurationFingerprint,
			}),
		).toThrow();
	});

	it("keeps provider work disabled until receipt and schema readiness are validated", () => {
		expect(core.safeManifestProjection(fixture.effectiveConfiguration as never)).toMatchObject({
			configurationFingerprint: fixture.effectiveConfiguration.configurationFingerprint,
			runtimeReason: "pending_schema_v21",
			schemaReadiness: "pending_schema_v21",
			packReadiness: "pending_pack_boundary",
			providerEnabled: false,
			sweeperEnabled: false,
		});
	});

	it("enables the provider, AI maintenance gate, and sweeper only at the ready projection", () => {
		const projection = core.safeManifestProjection(
			fixture.effectiveConfiguration as never,
			"available",
			"validated",
			"ready",
		);

		expect(projection).toMatchObject({
			runtimeReason: "ready",
			schemaReadiness: "ready",
			packReadiness: "ready",
			providerEnabled: true,
			sweeperEnabled: true,
			resourceProfile: {
				workerWarmLifetimeMs: 30_000,
				periodicSweepIntervalMs: 30_000,
				idleFlushMs: 120_000,
				eventDebounceMs: 1_000,
			},
		});
	});

	it.each([
		"provider_unavailable",
		"provider_tls_rejected",
	] as const)("keeps provider, AI maintenance, and sweeper disabled on %s", (providerHealth) => {
		expect(
			core.safeManifestProjection(
				fixture.effectiveConfiguration as never,
				providerHealth,
				"validated",
				"ready",
			),
		).toMatchObject({
			runtimeReason: providerHealth,
			providerHealth,
			providerEnabled: false,
			sweeperEnabled: false,
		});
	});

	it("keeps capture-only and unvalidated activation states disabled", () => {
		expect(core.captureOnlyCapabilityProjection("ready")).toMatchObject({
			mode: "capture_only",
			providerEnabled: false,
			sweeperEnabled: false,
		});
		for (const activationReceipt of ["absent", "rejected"] as const) {
			expect(
				core.safeManifestProjection(
					fixture.effectiveConfiguration as never,
					"available",
					activationReceipt,
					"ready",
				),
			).toMatchObject({
				activationReceipt,
				providerEnabled: false,
				sweeperEnabled: false,
			});
		}
	});
});

describe("Slice 1 provider TLS preflight", () => {
	const remoteProvider = fixture.effectiveConfiguration.summaryProvider as JsonObject;
	const localProvider = fixture.localDerivationManifest.summaryProvider as JsonObject;

	it("destroys a successful native TLS socket without waiting for peer FIN", async () => {
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		const socket = Object.assign(new EventEmitter(), {
			authorized: true,
			setTimeout: vi.fn(),
			getPeerCertificate: vi.fn(() => ({ raw: Buffer.from("test-peer-certificate") })),
			destroy: vi.fn(),
			end: vi.fn(),
		});
		nativeTlsConnect.run = () => {
			queueMicrotask(() => socket.emit("secureConnect"));
			return socket;
		};

		try {
			await expect(preflightProviderTls(localProvider, { environment: {} })).resolves.toEqual({
				peerCertificateSha256:
					"sha256:61b82c1e80433df70ed28e3ff769083b573612dd72e6d7a2177138b9335ba360",
			});
			expect(socket.destroy).toHaveBeenCalledOnce();
			expect(socket.end).not.toHaveBeenCalled();
		} finally {
			nativeTlsConnect.run = undefined;
		}
	});

	it("enforces the absolute native TLS deadline despite socket activity", async () => {
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		vi.useFakeTimers();
		let inactivityTimer: ReturnType<typeof setTimeout> | undefined;
		const socket = Object.assign(new EventEmitter(), {
			destroy: vi.fn(),
			setTimeout: vi.fn(),
		});
		socket.setTimeout.mockImplementation((timeoutMs: number) => {
			const resetInactivityTimer = () => {
				if (inactivityTimer) clearTimeout(inactivityTimer);
				inactivityTimer = setTimeout(() => socket.emit("timeout"), timeoutMs);
			};
			socket.on("activity", resetInactivityTimer);
			resetInactivityTimer();
			return socket;
		});
		let connectOptions: JsonObject | undefined;
		nativeTlsConnect.run = (options) => {
			connectOptions = options as JsonObject;
			return socket;
		};

		try {
			const rejection = expect(
				preflightProviderTls(localProvider, { environment: {} }),
			).rejects.toMatchObject({ reason: "provider_unavailable" });
			for (let second = 0; second < 4; second++) {
				await vi.advanceTimersByTimeAsync(1_000);
				socket.emit("activity");
			}
			await vi.advanceTimersByTimeAsync(999);
			expect(socket.destroy).not.toHaveBeenCalled();

			await vi.advanceTimersByTimeAsync(1);
			await rejection;
			expect(connectOptions).toEqual({
				host: "127.0.0.1",
				port: 1234,
				rejectUnauthorized: true,
			});
			expect(socket.destroy).toHaveBeenCalledOnce();

			await vi.runAllTimersAsync();
			expect(socket.destroy).toHaveBeenCalledOnce();
		} finally {
			nativeTlsConnect.run = undefined;
			if (inactivityTimer) clearTimeout(inactivityTimer);
			vi.useRealTimers();
		}
	});

	it("passes only endpoint identity and fixed TLS policy to the connector", async () => {
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		let observed: Parameters<TlsPreflightConnector>[0] | undefined;
		const result = await preflightProviderTls(remoteProvider, {
			environment: {},
			connect: async (input) => {
				observed = input;
				return {
					chainVerified: true,
					hostnameVerified: true,
					peerCertificateSha256: `sha256:${"a".repeat(64)}`,
				};
			},
		});

		expect(observed).toEqual({
			host: "summary.stub.invalid",
			port: 443,
			servername: "summary.stub.invalid",
			timeoutMs: 5_000,
			rejectUnauthorized: true,
		});
		expect(observed).not.toHaveProperty("credentialRef");
		expect(observed).not.toHaveProperty("headers");
		expect(observed).not.toHaveProperty("body");
		expect(observed).not.toHaveProperty("request");
		expect(result).toEqual({ peerCertificateSha256: `sha256:${"a".repeat(64)}` });
	});

	it("uses no SNI for an exact loopback IP", async () => {
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		let observed: Parameters<TlsPreflightConnector>[0] | undefined;

		await preflightProviderTls(localProvider, {
			environment: {},
			connect: async (input) => {
				observed = input;
				return {
					chainVerified: true,
					hostnameVerified: true,
					peerCertificateSha256: `sha256:${"d".repeat(64)}`,
				};
			},
		});

		expect(observed).toEqual({
			host: "127.0.0.1",
			port: 1234,
			servername: null,
			timeoutMs: 5_000,
			rejectUnauthorized: true,
		});
	});

	it.each([
		["chain", { chainVerified: false, hostnameVerified: true }],
		["hostname", { chainVerified: true, hostnameVerified: false }],
	])("rejects %s failure even when the peer reports the caller UID", async (_label, state) => {
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		const connect = (async () => ({
			...state,
			peerCertificateSha256: `sha256:${"b".repeat(64)}`,
			peerUid: typeof process.getuid === "function" ? process.getuid() : 0,
		})) as TlsPreflightConnector;

		try {
			await preflightProviderTls(localProvider, { environment: {}, connect });
			throw new Error("expected TLS rejection");
		} catch (error) {
			expect(error).toBeInstanceOf(ProviderTlsPreflightError);
			expect((error as ProviderTlsPreflightError).reason).toBe("provider_tls_rejected");
		}
	});

	it.each([
		"NODE_TLS_REJECT_UNAUTHORIZED",
		"NODE_EXTRA_CA_CERTS",
		"SSL_CERT_FILE",
		"SSL_CERT_DIR",
	])("rejects production TLS override %s before connecting", async (key) => {
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		let connectCalls = 0;
		const connect: TlsPreflightConnector = async () => {
			connectCalls += 1;
			return {
				chainVerified: true,
				hostnameVerified: true,
				peerCertificateSha256: `sha256:${"c".repeat(64)}`,
			};
		};

		await expect(
			preflightProviderTls(remoteProvider, {
				environment: { [key]: key === "NODE_TLS_REJECT_UNAUTHORIZED" ? "0" : "/tmp/test-ca" },
				connect,
			}),
		).rejects.toThrow();
		expect(connectCalls).toBe(0);
	});

	it("does not call TLS for credential-none local HTTP", async () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		const provider = compileProviderChoice({
			...providerProposal(localProvider),
			endpointUrl: "http://127.0.0.1:1234/v1/chat/completions",
			credentialRef: { kind: "none" },
		});
		let connectCalls = 0;

		await expect(
			preflightProviderTls(provider, {
				environment: {},
				connect: async () => {
					connectCalls += 1;
					throw new Error("local HTTP must not attempt TLS");
				},
			}),
		).resolves.toEqual({ skipped: "local_http" });
		expect(connectCalls).toBe(0);
	});

	it("derives unverified HTTP and verified HTTPS provider trust from preflight evidence", async () => {
		const compileProviderChoice = publicFunction<CompileProviderChoice>("compileProviderChoice");
		const preflightProviderTls = publicFunction<PreflightProviderTls>("preflightProviderTls");
		const repository = `repo-v1:sha256:${"e".repeat(64)}`;
		const localHttp = compileProviderChoice({
			...providerProposal(localProvider),
			endpointUrl: "http://127.0.0.1:1234/v1/chat/completions",
			credentialRef: { kind: "none" },
		});
		const httpPreflight = await preflightProviderTls(localHttp, { environment: {} });
		const httpsPreflight = await preflightProviderTls(localProvider, {
			environment: {},
			connect: async () => ({
				chainVerified: true,
				hostnameVerified: true,
				peerCertificateSha256: `sha256:${"f".repeat(64)}`,
			}),
		});
		const httpBoundary = compileProviderDestinationBoundary(
			core.compileDefaultCapabilityManifest(providerProposal(localHttp) as never),
			{ repositoryIdentity: repository, tlsPeerVerified: false },
		);
		const httpsBoundary = compileProviderDestinationBoundary(
			fixture.localDerivationManifest as never,
			{
				repositoryIdentity: repository,
				tlsPeerVerified: "peerCertificateSha256" in httpsPreflight,
			},
		);

		expect(httpPreflight).toEqual({ skipped: "local_http" });
		expect(httpBoundary.providerPeerTrust).toBe("unverified");
		expect(
			isDestinationEligible(httpBoundary, {
				sensitivity: "private",
				repositoryIdentity: repository,
			}),
		).toBe(false);
		expect(httpsBoundary.providerPeerTrust).toBe("verified");
		expect(
			isDestinationEligible(httpsBoundary, {
				sensitivity: "private",
				repositoryIdentity: repository,
			}),
		).toBe(true);
	});
});
