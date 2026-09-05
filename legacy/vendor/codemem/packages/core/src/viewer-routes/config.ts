import { Hono } from "hono";
import { captureOnlyCapabilityProjection } from "../capability-manifest.js";

export function configReadRoutes(deps?: { getCapabilitySnapshot?: () => Record<string, unknown> }) {
	const app = new Hono();
	app.get("/api/config", (c) => {
		const capability = (deps?.getCapabilitySnapshot?.() ??
			captureOnlyCapabilityProjection()) as Record<string, unknown>;
		const provider = capability.summaryProvider;
		const profile = capability.resourceProfile;
		const providerRecord =
			provider && typeof provider === "object" && !Array.isArray(provider)
				? (provider as Record<string, unknown>)
				: null;
		const profileRecord =
			profile && typeof profile === "object" && !Array.isArray(profile)
				? (profile as Record<string, unknown>)
				: null;
		let providerName: "anthropic" | "openai" | null = null;
		if (providerRecord?.wireProtocol === "anthropic_messages_v1") {
			providerName = "anthropic";
		} else if (providerRecord?.wireProtocol === "openai_chat_completions_v1") {
			providerName = "openai";
		}
		const effective = providerRecord
			? {
					observer_provider: providerName,
					observer_model: providerRecord.modelId,
					observer_base_url: providerRecord.endpointUrl,
					observer_temperature: profileRecord?.observerTemperature,
					observer_max_chars: profileRecord?.observerMaxInputChars,
					observer_max_output_tokens: profileRecord?.observerMaxOutputTokens,
				}
			: {};
		return c.json({
			path: "",
			config: {},
			defaults: {},
			effective,
			env_overrides: {},
			providers: providerName ? [providerName] : [],
			capability,
		});
	});
	return app;
}
