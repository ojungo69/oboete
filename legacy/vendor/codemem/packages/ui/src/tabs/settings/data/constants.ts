/* Settings modal constants — tab ids, localStorage keys, defaults,
 * config-key mapping tables, and the empty form state. */

import type { RadixTabOption } from "../../../components/primitives/radix-tabs";
import type { SettingsFormState } from "./types";

export const SETTINGS_ADVANCED_KEY = "codemem-settings-advanced";

export const DEFAULT_OPENAI_MODEL = "gpt-5.1-codex-mini";
export const DEFAULT_ANTHROPIC_MODEL = "claude-4.5-haiku";

export const SETTINGS_TABS: RadixTabOption[] = [
	{ value: "observer", label: "Connection" },
	{ value: "queue", label: "Processing" },
];

export const INPUT_TO_CONFIG_KEY: Record<keyof SettingsFormState, string> = {
	observerProvider: "observer_provider",
	observerModel: "observer_model",
	observerTierRoutingEnabled: "observer_tier_routing_enabled",
	observerSimpleModel: "observer_simple_model",
	observerSimpleTemperature: "observer_simple_temperature",
	observerReasoningEffort: "observer_reasoning_effort",
	observerReasoningSummary: "observer_reasoning_summary",
	observerRichModel: "observer_rich_model",
	observerRichTemperature: "observer_rich_temperature",
	observerRichReasoningEffort: "observer_rich_reasoning_effort",
	observerRichReasoningSummary: "observer_rich_reasoning_summary",
	observerRichMaxOutputTokens: "observer_rich_max_output_tokens",
	observerAuthSource: "observer_auth_source",
	observerAuthFile: "observer_auth_file",
	observerAuthCacheTtlS: "observer_auth_cache_ttl_s",
	observerHeaders: "observer_headers",
	observerMaxChars: "observer_max_chars",
	packObservationLimit: "pack_observation_limit",
	packSessionLimit: "pack_session_limit",
	rawEventsSweeperIntervalS: "raw_events_sweeper_interval_s",
};

export const PROTECTED_VIEWER_CONFIG_KEYS = new Set([
	"observer_base_url",
	"observer_auth_file",
	"observer_headers",
]);

export const EMPTY_FORM_STATE: SettingsFormState = {
	observerProvider: "",
	observerModel: "",
	observerTierRoutingEnabled: false,
	observerSimpleModel: "",
	observerSimpleTemperature: "",
	observerReasoningEffort: "",
	observerReasoningSummary: "",
	observerRichModel: "",
	observerRichTemperature: "",
	observerRichReasoningEffort: "",
	observerRichReasoningSummary: "",
	observerRichMaxOutputTokens: "",
	observerAuthSource: "auto",
	observerAuthFile: "",
	observerAuthCacheTtlS: "",
	observerHeaders: "",
	observerMaxChars: "",
	packObservationLimit: "",
	packSessionLimit: "",
	rawEventsSweeperIntervalS: "",
};
