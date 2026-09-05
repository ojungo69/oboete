/* Flatten a SettingsFormState back into the config payload shape the
 * viewer expects. Validates JSON-array / number / temperature fields
 * and throws labeled errors for the save flow to surface. */

import { parseObserverHeaders } from "./parse";
import type { SettingsFormState } from "./types";
import { normalizeTextValue } from "./value-helpers";

export interface CollectSettingsPayloadInput {
	values: SettingsFormState;
	touchedKeys: Set<string>;
	baseline: Record<string, unknown>;
	allowUntouchedParseErrors?: boolean;
}

export function collectSettingsPayload(
	input: CollectSettingsPayloadInput,
): Record<string, unknown> {
	const { values, touchedKeys, baseline } = input;
	const allowUntouchedParseErrors = input.allowUntouchedParseErrors === true;
	let headers: Record<string, string> = {};
	try {
		headers = parseObserverHeaders(values.observerHeaders);
	} catch (error) {
		if (!allowUntouchedParseErrors || touchedKeys.has("observer_headers")) {
			throw error;
		}
		const baselineValue = baseline.observer_headers;
		if (baselineValue && typeof baselineValue === "object" && !Array.isArray(baselineValue)) {
			Object.entries(baselineValue as Record<string, unknown>).forEach(([key, value]) => {
				if (typeof key === "string" && key.trim() && typeof value === "string") {
					headers[key] = value;
				}
			});
		}
	}

	const authCacheTtlInput = values.observerAuthCacheTtlS.trim();
	const simpleTemperatureInput = values.observerSimpleTemperature.trim();
	const richTemperatureInput = values.observerRichTemperature.trim();
	const richMaxOutputTokensInput = values.observerRichMaxOutputTokens.trim();
	const sweeperIntervalInput = values.rawEventsSweeperIntervalS.trim();
	const authCacheTtl = authCacheTtlInput === "" ? "" : Number(authCacheTtlInput);
	const simpleTemperature = simpleTemperatureInput === "" ? "" : Number(simpleTemperatureInput);
	const richTemperature = richTemperatureInput === "" ? "" : Number(richTemperatureInput);
	const richMaxOutputTokens =
		richMaxOutputTokensInput === "" ? "" : Number(richMaxOutputTokensInput);
	const sweeperIntervalNum = Number(sweeperIntervalInput);
	const sweeperInterval = sweeperIntervalInput === "" ? "" : sweeperIntervalNum;

	if (authCacheTtlInput !== "" && !Number.isFinite(authCacheTtl)) {
		throw new Error("observer auth cache ttl must be a number");
	}
	if (
		simpleTemperatureInput !== "" &&
		(typeof simpleTemperature !== "number" ||
			!Number.isFinite(simpleTemperature) ||
			simpleTemperature < 0)
	) {
		throw new Error("simple tier temperature must be a non-negative number");
	}
	if (
		richTemperatureInput !== "" &&
		(typeof richTemperature !== "number" ||
			!Number.isFinite(richTemperature) ||
			richTemperature < 0)
	) {
		throw new Error("rich tier temperature must be a non-negative number");
	}
	if (
		richMaxOutputTokensInput !== "" &&
		(typeof richMaxOutputTokens !== "number" ||
			!Number.isFinite(richMaxOutputTokens) ||
			richMaxOutputTokens <= 0 ||
			!Number.isInteger(richMaxOutputTokens))
	) {
		throw new Error("rich tier max output tokens must be a positive integer");
	}
	if (
		sweeperIntervalInput !== "" &&
		(!Number.isFinite(sweeperIntervalNum) || sweeperIntervalNum <= 0)
	) {
		throw new Error("raw-event sweeper interval must be a positive number");
	}

	return {
		observer_provider: normalizeTextValue(values.observerProvider),
		observer_model: normalizeTextValue(values.observerModel),
		observer_tier_routing_enabled: values.observerTierRoutingEnabled,
		observer_simple_model: normalizeTextValue(values.observerSimpleModel),
		observer_simple_temperature: simpleTemperature,
		observer_reasoning_effort: normalizeTextValue(values.observerReasoningEffort),
		observer_reasoning_summary: normalizeTextValue(values.observerReasoningSummary),
		observer_rich_model: normalizeTextValue(values.observerRichModel),
		observer_rich_temperature: richTemperature,
		observer_rich_reasoning_effort: normalizeTextValue(values.observerRichReasoningEffort),
		observer_rich_reasoning_summary: normalizeTextValue(values.observerRichReasoningSummary),
		observer_rich_max_output_tokens: richMaxOutputTokens,
		observer_auth_source: normalizeTextValue(values.observerAuthSource || "auto") || "auto",
		observer_auth_file: normalizeTextValue(values.observerAuthFile),
		observer_auth_cache_ttl_s: authCacheTtl,
		observer_headers: headers,
		observer_max_chars: Number(values.observerMaxChars || 0) || "",
		pack_observation_limit: Number(values.packObservationLimit || 0) || "",
		pack_session_limit: Number(values.packSessionLimit || 0) || "",
		raw_events_sweeper_interval_s: sweeperInterval,
	};
}
