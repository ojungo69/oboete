/* Convert a viewer /settings config payload into the flat
 * SettingsFormState the UI renders from. Pure — no module state. */

import type { SettingsFormState } from "./types";
import { asBooleanValue, asInputString, effectiveOrConfigured } from "./value-helpers";

export interface ConfigPayload {
	config?: Record<string, unknown>;
	effective?: Record<string, unknown>;
	defaults?: Record<string, unknown>;
	env_overrides?: Record<string, unknown>;
	protected_keys?: unknown;
	providers?: unknown;
	path?: string;
}

export function formStateFromPayload(payload: ConfigPayload): SettingsFormState {
	const config = payload.config || {};
	const effective = payload.effective || {};
	const observerHeadersValue = effectiveOrConfigured(config, effective, "observer_headers");
	const observerHeaders =
		observerHeadersValue &&
		typeof observerHeadersValue === "object" &&
		!Array.isArray(observerHeadersValue)
			? Object.fromEntries(
					Object.entries(observerHeadersValue as Record<string, unknown>).filter(
						([key, value]) => typeof key === "string" && key.trim() && typeof value === "string",
					),
				)
			: {};
	return {
		observerProvider: asInputString(effectiveOrConfigured(config, effective, "observer_provider")),
		observerModel: asInputString(effectiveOrConfigured(config, effective, "observer_model")),
		observerTierRoutingEnabled: asBooleanValue(
			effectiveOrConfigured(config, effective, "observer_tier_routing_enabled"),
		),
		observerSimpleModel: asInputString(
			effectiveOrConfigured(config, effective, "observer_simple_model"),
		),
		observerSimpleTemperature: asInputString(
			effectiveOrConfigured(config, effective, "observer_simple_temperature"),
		),
		observerReasoningEffort: asInputString(
			effectiveOrConfigured(config, effective, "observer_reasoning_effort"),
		),
		observerReasoningSummary: asInputString(
			effectiveOrConfigured(config, effective, "observer_reasoning_summary"),
		),
		observerRichModel: asInputString(
			effectiveOrConfigured(config, effective, "observer_rich_model"),
		),
		observerRichTemperature: asInputString(
			effectiveOrConfigured(config, effective, "observer_rich_temperature"),
		),
		observerRichReasoningEffort: asInputString(
			effectiveOrConfigured(config, effective, "observer_rich_reasoning_effort"),
		),
		observerRichReasoningSummary: asInputString(
			effectiveOrConfigured(config, effective, "observer_rich_reasoning_summary"),
		),
		observerRichMaxOutputTokens: asInputString(
			effectiveOrConfigured(config, effective, "observer_rich_max_output_tokens"),
		),
		observerAuthSource:
			asInputString(effectiveOrConfigured(config, effective, "observer_auth_source")) || "auto",
		observerAuthFile: asInputString(effectiveOrConfigured(config, effective, "observer_auth_file")),
		observerAuthCacheTtlS: asInputString(
			effectiveOrConfigured(config, effective, "observer_auth_cache_ttl_s"),
		),
		observerHeaders: Object.keys(observerHeaders).length
			? JSON.stringify(observerHeaders, null, 2)
			: "",
		observerMaxChars: asInputString(effectiveOrConfigured(config, effective, "observer_max_chars")),
		packObservationLimit: asInputString(
			effectiveOrConfigured(config, effective, "pack_observation_limit"),
		),
		packSessionLimit: asInputString(effectiveOrConfigured(config, effective, "pack_session_limit")),
		rawEventsSweeperIntervalS: asInputString(
			effectiveOrConfigured(config, effective, "raw_events_sweeper_interval_s"),
		),
	};
}
