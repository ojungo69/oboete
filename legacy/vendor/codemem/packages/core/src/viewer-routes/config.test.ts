import { describe, expect, it } from "vitest";
import { configReadRoutes } from "./config.js";

const frozenProjection = {
	configurationFingerprint:
		"sha256:2a5a5d2d3803d8f2dc2767981cbbf4f77cffc3aae8cebdc9d310e7645b27d53d",
	summaryProvider: {
		providerFingerprint: "sha256:d184deae938722877e017d85ab382a4f72c287857bf0f346f483263680635ede",
		wireProtocol: "openai_chat_completions_v1",
		modelId: "deterministic-summary-model-v1",
		modelRevision: "1",
		endpointUrl: "https://summary.stub.invalid/v1/chat/completions",
		credentialRef: { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" },
		executionLocation: "remote",
		egressPolicy: "explicit_remote",
		costClass: "external_metered",
		tlsPolicy: "system",
		redirectPolicy: "reject",
	},
	embeddingProvider: {
		state: "disabled",
		reason: "slice1_semantic_not_owned",
		packDegradationReason: "semantic_disabled",
	},
	readiness: "pending_privacy_boundary",
} as const;

type ConfigRouteFactory = (deps: {
	getCapabilitySnapshot: () => typeof frozenProjection;
}) => ReturnType<typeof configReadRoutes>;

describe("viewer capability configuration", () => {
	it("returns the frozen safe projection and ignores later legacy environment changes", async () => {
		const environment = {
			CODEMEM_OBSERVER_API_KEY: process.env.CODEMEM_OBSERVER_API_KEY,
			CODEMEM_OBSERVER_BASE_URL: process.env.CODEMEM_OBSERVER_BASE_URL,
			CODEMEM_OBSERVER_HEADERS: process.env.CODEMEM_OBSERVER_HEADERS,
		};

		try {
			process.env.CODEMEM_OBSERVER_API_KEY = "fixture-viewer-legacy-token";
			process.env.CODEMEM_OBSERVER_BASE_URL = "https://legacy.invalid/v1";
			process.env.CODEMEM_OBSERVER_HEADERS = JSON.stringify({ "x-legacy": "must-not-render" });
			const createRoutes = configReadRoutes as unknown as ConfigRouteFactory;
			const routes = createRoutes({ getCapabilitySnapshot: () => frozenProjection });

			const response = await routes.request("/api/config");
			const body = (await response.json()) as Record<string, unknown>;

			expect(response.status).toBe(200);
			expect(body.capability).toEqual(frozenProjection);
			expect(body).toMatchObject({
				path: "",
				config: {},
				defaults: {},
				effective: {
					observer_provider: "openai",
					observer_model: "deterministic-summary-model-v1",
					observer_base_url: "https://summary.stub.invalid/v1/chat/completions",
				},
				env_overrides: {},
				providers: ["openai"],
			});
			expect(JSON.stringify(body)).not.toContain("fixture-viewer-legacy-token");
			expect(JSON.stringify(body)).not.toContain("legacy.invalid");
			expect(JSON.stringify(body)).not.toContain("must-not-render");
		} finally {
			for (const [key, value] of Object.entries(environment)) {
				if (value === undefined) delete process.env[key];
				else process.env[key] = value;
			}
		}
	});
});
