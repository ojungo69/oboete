/**
 * Observer client: LLM caller for analyzing coding session transcripts.
 *
 * Mirrors codemem/observer.py — resolves provider config + auth, then calls
 * an LLM (Anthropic Messages or OpenAI Chat Completions) via fetch to extract
 * memories from session transcripts.
 *
 * Supports direct API HTTP requests (no sidecar runtimes).
 * Non-streaming responses via fetch (no SDK deps).
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import {
	type ProviderChoiceV1,
	type ProviderTransportProfileV1,
	validateProviderChoice,
	validateProviderTransportProfile,
} from "./capability-manifest.js";
import {
	type DestinationBoundaryV1,
	destinationBoundaryFingerprint,
} from "./destination-boundary.js";
import { codememHomeDir } from "./home.js";

import {
	ObserverAuthAdapter,
	type ObserverAuthMaterial,
	renderObserverHeaders,
} from "./observer-auth.js";
import {
	getOpenCodeProviderConfig,
	getProviderApiKey,
	listConfiguredOpenCodeProviders,
	resolveBuiltInProviderDefaultModel,
	resolveBuiltInProviderFromModel,
	resolveBuiltInProviderModel,
	resolveCustomProviderDefaultModel,
	resolveCustomProviderFromModel,
	resolveCustomProviderModel,
	stripJsonComments,
	stripTrailingCommas,
} from "./observer-config.js";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_ANTHROPIC_MODEL = "claude-haiku-4-5";
const DEFAULT_OPENAI_MODEL = "gpt-5.4-mini";

const ANTHROPIC_MESSAGES_ENDPOINT = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION = "2023-06-01";

const FETCH_TIMEOUT_MS = 60_000;

function stripTrailingSlashes(value: string): string {
	let end = value.length;
	while (end > 0 && value.charCodeAt(end - 1) === 47) end--;
	return end === value.length ? value : value.slice(0, end);
}

// Anthropic model name aliases (friendly → API id)
const ANTHROPIC_MODEL_ALIASES: Record<string, string> = {
	"claude-4.5-haiku": "claude-haiku-4-5",
	"claude-4.5-sonnet": "claude-sonnet-4-5",
	"claude-4.5-opus": "claude-opus-4-5",
	"claude-4.6-sonnet": "claude-sonnet-4-6",
	"claude-4.6-opus": "claude-opus-4-6",
	"claude-4.1-opus": "claude-opus-4-1",
	"claude-4.0-sonnet": "claude-sonnet-4-0",
	"claude-4.0-opus": "claude-opus-4-0",
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface ObserverConfig {
	observerProvider: string | null;
	observerModel: string | null;
	observerRuntime: string | null;
	observerApiKey: string | null;
	observerBaseUrl: string | null;
	observerTemperature?: number | null;
	observerTierRoutingEnabled?: boolean;
	observerSimpleProvider?: string | null;
	observerSimpleModel?: string | null;
	observerSimpleTemperature?: number | null;
	observerRichProvider?: string | null;
	observerRichModel?: string | null;
	observerRichTemperature?: number | null;
	observerRichReasoningEffort?: string | null;
	observerRichReasoningSummary?: string | null;
	observerRichMaxOutputTokens?: number | null;
	observerOpenAIUseResponses?: boolean;
	observerReasoningEffort?: string | null;
	observerReasoningSummary?: string | null;
	observerMaxOutputTokens?: number | null;
	observerMaxChars: number;
	observerMaxTokens: number;
	observerHeaders: Record<string, string>;
	observerAuthSource: string;
	observerAuthFile: string | null;
	observerAuthCacheTtlS: number;
	observerExplicitConfigKeys?: string[];
}

export interface ObserverResponse {
	raw: string | null;
	parsed: Record<string, unknown> | null;
	provider: string;
	model: string;
	/** Wall-clock duration for this invocation, including an auth retry when needed. */
	elapsedMs?: number;
	/** Provider-reported token usage. Null when the transport does not expose it. */
	usage?: ObserverTokenUsage | null;
}

export interface ObserverTokenUsage {
	inputTokens: number;
	outputTokens: number;
	totalTokens?: number;
	cacheReadInputTokens?: number;
	cacheCreationInputTokens?: number;
}

export interface ObserverStructuredJsonResponse extends ObserverResponse {
	usedStructuredOutputs: boolean;
}

type ObserverDiagnosticReason =
	| "provider_unavailable"
	| "provider_redirect_rejected"
	| "provider_tls_rejected"
	| "provider_auth_failed"
	| "output_invalid"
	| "output_limit_exceeded";

type ObserverDiagnosticNextAction =
	| "none"
	| "activate_valid_manifest"
	| "configure_credential"
	| "confirm_retry";

export type ObserverTransportProfile = ProviderTransportProfileV1;

export interface ObserverStatus {
	provider: string;
	model: string;
	runtime: string;
	auth: { source: string; type: string; hasToken: boolean };
	actualModel?: string | null;
	modelFallbackApplied?: boolean;
	modelFallbackReason?: string | null;
	lastError?: {
		version: 1;
		action: "failed";
		reason: ObserverDiagnosticReason;
		destination: "local" | "remote" | "unknown";
		counts: {
			considered: number;
			transmitted: number;
			eligible: number;
			localOnly: number;
			private: number;
			secret: number;
		};
		configurationFingerprint: string | null;
		providerFingerprint: string | null;
		nextAction: ObserverDiagnosticNextAction;
		/** Legacy non-enumerable compatibility alias. */
		code?: string;
		/** Removed from runtime diagnostics; retained only for source compatibility. */
		message?: never;
	} | null;
}

interface ObserverConfigKeyMapping {
	fileKey: string;
	envKey: string;
	normalizedKey: keyof ObserverConfig;
}

const OBSERVER_CONFIG_KEY_MAPPINGS: ObserverConfigKeyMapping[] = [
	{
		fileKey: "observer_tier_routing_enabled",
		envKey: "CODEMEM_OBSERVER_TIER_ROUTING_ENABLED",
		normalizedKey: "observerTierRoutingEnabled",
	},
	{
		fileKey: "observer_provider",
		envKey: "CODEMEM_OBSERVER_PROVIDER",
		normalizedKey: "observerProvider",
	},
	{ fileKey: "observer_model", envKey: "CODEMEM_OBSERVER_MODEL", normalizedKey: "observerModel" },
	{
		fileKey: "observer_simple_provider",
		envKey: "CODEMEM_OBSERVER_SIMPLE_PROVIDER",
		normalizedKey: "observerSimpleProvider",
	},
	{
		fileKey: "observer_simple_model",
		envKey: "CODEMEM_OBSERVER_SIMPLE_MODEL",
		normalizedKey: "observerSimpleModel",
	},
	{
		fileKey: "observer_simple_temperature",
		envKey: "CODEMEM_OBSERVER_SIMPLE_TEMPERATURE",
		normalizedKey: "observerSimpleTemperature",
	},
	{
		fileKey: "observer_rich_provider",
		envKey: "CODEMEM_OBSERVER_RICH_PROVIDER",
		normalizedKey: "observerRichProvider",
	},
	{
		fileKey: "observer_rich_model",
		envKey: "CODEMEM_OBSERVER_RICH_MODEL",
		normalizedKey: "observerRichModel",
	},
	{
		fileKey: "observer_rich_temperature",
		envKey: "CODEMEM_OBSERVER_RICH_TEMPERATURE",
		normalizedKey: "observerRichTemperature",
	},
	{
		fileKey: "observer_rich_reasoning_effort",
		envKey: "CODEMEM_OBSERVER_RICH_REASONING_EFFORT",
		normalizedKey: "observerRichReasoningEffort",
	},
	{
		fileKey: "observer_rich_reasoning_summary",
		envKey: "CODEMEM_OBSERVER_RICH_REASONING_SUMMARY",
		normalizedKey: "observerRichReasoningSummary",
	},
	{
		fileKey: "observer_rich_max_output_tokens",
		envKey: "CODEMEM_OBSERVER_RICH_MAX_OUTPUT_TOKENS",
		normalizedKey: "observerRichMaxOutputTokens",
	},
	{
		fileKey: "observer_openai_use_responses",
		envKey: "CODEMEM_OBSERVER_OPENAI_USE_RESPONSES",
		normalizedKey: "observerOpenAIUseResponses",
	},
	{
		fileKey: "observer_reasoning_effort",
		envKey: "CODEMEM_OBSERVER_REASONING_EFFORT",
		normalizedKey: "observerReasoningEffort",
	},
	{
		fileKey: "observer_reasoning_summary",
		envKey: "CODEMEM_OBSERVER_REASONING_SUMMARY",
		normalizedKey: "observerReasoningSummary",
	},
	{
		fileKey: "observer_max_output_tokens",
		envKey: "CODEMEM_OBSERVER_MAX_OUTPUT_TOKENS",
		normalizedKey: "observerMaxOutputTokens",
	},
];

function collectExplicitObserverConfigKeys(
	data: Record<string, unknown>,
	env: NodeJS.ProcessEnv,
): string[] {
	const keys = new Set<string>();
	for (const { fileKey, envKey, normalizedKey } of OBSERVER_CONFIG_KEY_MAPPINGS) {
		if (fileKey in data || env[envKey] != null) keys.add(normalizedKey);
	}
	return [...keys];
}

function resolveExplicitObserverConfigKeys(
	cfg: ObserverConfig,
	configWasProvided: boolean,
): Set<string> {
	if (Array.isArray(cfg.observerExplicitConfigKeys)) {
		return new Set(cfg.observerExplicitConfigKeys);
	}
	if (!configWasProvided) return new Set();
	return new Set(
		Object.entries(cfg)
			.filter(([, value]) => value !== undefined)
			.map(([key]) => key),
	);
}

function supportsDefaultTierRouting(
	provider: string,
	runtime: string,
	hasCustomBaseUrl: boolean,
): boolean {
	if (runtime !== "api_http") return false;
	if (provider !== "openai" && provider !== "anthropic") return false;
	// A custom base URL may point at an OpenAI-compatible gateway that only
	// implements chat/completions. Rich-tier defaults turn Responses on, so we
	// cannot assume capability-safety without an explicit user opt-in.
	if (hasCustomBaseUrl) return false;
	return true;
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

function parseIntSafe(value: unknown, fallback: number): number {
	if (value == null) return fallback;
	const n = typeof value === "string" ? Number.parseInt(value, 10) : Number(value);
	return Number.isFinite(n) ? n : fallback;
}

function coerceStringMap(value: unknown): Record<string, string> | null {
	if (value == null) return null;
	if (typeof value === "string") {
		const trimmed = value.trim();
		if (!trimmed) return {};
		try {
			const parsed = JSON.parse(trimmed);
			if (typeof parsed !== "object" || parsed == null || Array.isArray(parsed)) return null;
			return parsed as Record<string, string>;
		} catch {
			return null;
		}
	}
	if (typeof value === "object" && !Array.isArray(value)) {
		return value as Record<string, string>;
	}
	return null;
}

/**
 * Load observer config from `~/.config/codemem/config.json{c}`.
 *
 * Reads the codemem config file (not OpenCode's) and extracts observer-related
 * fields with environment variable overrides.
 */
export function loadObserverConfig(): ObserverConfig {
	const defaults: ObserverConfig = {
		observerProvider: null,
		observerModel: null,
		observerRuntime: "api_http",
		observerApiKey: null,
		observerBaseUrl: null,
		observerTemperature: 0.2,
		observerTierRoutingEnabled: false,
		observerSimpleProvider: null,
		observerSimpleModel: null,
		observerSimpleTemperature: null,
		observerRichProvider: null,
		observerRichModel: null,
		observerRichTemperature: null,
		observerRichReasoningEffort: null,
		observerRichReasoningSummary: null,
		observerRichMaxOutputTokens: null,
		observerOpenAIUseResponses: undefined,
		observerReasoningEffort: null,
		observerReasoningSummary: null,
		observerMaxOutputTokens: null,
		observerMaxChars: 12_000,
		observerMaxTokens: 4_000,
		observerHeaders: {},
		observerAuthSource: "auto",
		observerAuthFile: null,
		observerAuthCacheTtlS: 300,
	};

	// Read config file
	const configDir = join(codememHomeDir(), ".config", "codemem");
	const envPath = process.env.CODEMEM_CONFIG;
	let configPath: string | null = null;
	if (envPath) {
		configPath = envPath.replace(/^~/, codememHomeDir());
	} else {
		const candidates = [join(configDir, "config.json"), join(configDir, "config.jsonc")];
		configPath = candidates.find((p) => existsSync(p)) ?? null;
	}

	let data: Record<string, unknown> = {};
	if (configPath && existsSync(configPath)) {
		try {
			let text = readFileSync(configPath, "utf-8");
			if (text.trim()) {
				try {
					data = JSON.parse(text) as Record<string, unknown>;
				} catch {
					text = stripTrailingCommas(stripJsonComments(text));
					data = JSON.parse(text) as Record<string, unknown>;
				}
				if (typeof data !== "object" || data == null || Array.isArray(data)) {
					data = {};
				}
			}
		} catch {
			data = {};
		}
	}

	// Apply config file values
	const cfg = { ...defaults };

	if (typeof data.observer_provider === "string") cfg.observerProvider = data.observer_provider;
	if (typeof data.observer_model === "string") cfg.observerModel = data.observer_model;
	if (typeof data.observer_api_key === "string") cfg.observerApiKey = data.observer_api_key;
	if (typeof data.observer_base_url === "string") cfg.observerBaseUrl = data.observer_base_url;
	if (data.observer_temperature != null) {
		const n = Number(data.observer_temperature);
		cfg.observerTemperature = Number.isFinite(n) ? n : cfg.observerTemperature;
	}
	if (data.observer_tier_routing_enabled != null) {
		cfg.observerTierRoutingEnabled = data.observer_tier_routing_enabled === true;
	}
	if (typeof data.observer_simple_provider === "string")
		cfg.observerSimpleProvider = data.observer_simple_provider;
	if (typeof data.observer_simple_model === "string")
		cfg.observerSimpleModel = data.observer_simple_model;
	if (data.observer_simple_temperature != null) {
		const n = Number(data.observer_simple_temperature);
		cfg.observerSimpleTemperature = Number.isFinite(n) ? n : cfg.observerSimpleTemperature;
	}
	if (typeof data.observer_rich_provider === "string")
		cfg.observerRichProvider = data.observer_rich_provider;
	if (typeof data.observer_rich_model === "string")
		cfg.observerRichModel = data.observer_rich_model;
	if (data.observer_rich_temperature != null) {
		const n = Number(data.observer_rich_temperature);
		cfg.observerRichTemperature = Number.isFinite(n) ? n : cfg.observerRichTemperature;
	}
	if (typeof data.observer_rich_reasoning_effort === "string") {
		cfg.observerRichReasoningEffort = data.observer_rich_reasoning_effort;
	}
	if (typeof data.observer_rich_reasoning_summary === "string") {
		cfg.observerRichReasoningSummary = data.observer_rich_reasoning_summary;
	}
	if (data.observer_rich_max_output_tokens != null) {
		const n = Number(data.observer_rich_max_output_tokens);
		cfg.observerRichMaxOutputTokens = Number.isFinite(n) ? n : cfg.observerRichMaxOutputTokens;
	}
	if (data.observer_openai_use_responses != null) {
		cfg.observerOpenAIUseResponses = data.observer_openai_use_responses === true;
	}
	if (typeof data.observer_reasoning_effort === "string") {
		cfg.observerReasoningEffort = data.observer_reasoning_effort;
	}
	if (typeof data.observer_reasoning_summary === "string") {
		cfg.observerReasoningSummary = data.observer_reasoning_summary;
	}
	if (data.observer_max_output_tokens != null) {
		const n = Number(data.observer_max_output_tokens);
		cfg.observerMaxOutputTokens = Number.isFinite(n) ? n : cfg.observerMaxOutputTokens;
	}
	cfg.observerMaxChars = parseIntSafe(data.observer_max_chars, cfg.observerMaxChars);
	cfg.observerMaxTokens = parseIntSafe(data.observer_max_tokens, cfg.observerMaxTokens);
	if (typeof data.observer_auth_source === "string")
		cfg.observerAuthSource = data.observer_auth_source;
	if (typeof data.observer_auth_file === "string") cfg.observerAuthFile = data.observer_auth_file;
	cfg.observerAuthCacheTtlS = parseIntSafe(
		data.observer_auth_cache_ttl_s,
		cfg.observerAuthCacheTtlS,
	);

	const headers = coerceStringMap(data.observer_headers);
	if (headers) cfg.observerHeaders = headers;

	// Apply env var overrides (take precedence over file)
	cfg.observerProvider = process.env.CODEMEM_OBSERVER_PROVIDER ?? cfg.observerProvider;
	cfg.observerModel = process.env.CODEMEM_OBSERVER_MODEL ?? cfg.observerModel;
	cfg.observerApiKey = process.env.CODEMEM_OBSERVER_API_KEY ?? cfg.observerApiKey;
	cfg.observerBaseUrl = process.env.CODEMEM_OBSERVER_BASE_URL ?? cfg.observerBaseUrl;
	if (process.env.CODEMEM_OBSERVER_TEMPERATURE != null) {
		const n = Number(process.env.CODEMEM_OBSERVER_TEMPERATURE);
		cfg.observerTemperature = Number.isFinite(n) ? n : cfg.observerTemperature;
	}
	if (process.env.CODEMEM_OBSERVER_TIER_ROUTING_ENABLED != null) {
		cfg.observerTierRoutingEnabled =
			process.env.CODEMEM_OBSERVER_TIER_ROUTING_ENABLED === "1" ||
			process.env.CODEMEM_OBSERVER_TIER_ROUTING_ENABLED === "true";
	}
	cfg.observerSimpleProvider =
		process.env.CODEMEM_OBSERVER_SIMPLE_PROVIDER ?? cfg.observerSimpleProvider;
	cfg.observerSimpleModel = process.env.CODEMEM_OBSERVER_SIMPLE_MODEL ?? cfg.observerSimpleModel;
	if (process.env.CODEMEM_OBSERVER_SIMPLE_TEMPERATURE != null) {
		const n = Number(process.env.CODEMEM_OBSERVER_SIMPLE_TEMPERATURE);
		cfg.observerSimpleTemperature = Number.isFinite(n) ? n : cfg.observerSimpleTemperature;
	}
	cfg.observerRichProvider = process.env.CODEMEM_OBSERVER_RICH_PROVIDER ?? cfg.observerRichProvider;
	cfg.observerRichModel = process.env.CODEMEM_OBSERVER_RICH_MODEL ?? cfg.observerRichModel;
	if (process.env.CODEMEM_OBSERVER_RICH_TEMPERATURE != null) {
		const n = Number(process.env.CODEMEM_OBSERVER_RICH_TEMPERATURE);
		cfg.observerRichTemperature = Number.isFinite(n) ? n : cfg.observerRichTemperature;
	}
	cfg.observerRichReasoningEffort =
		process.env.CODEMEM_OBSERVER_RICH_REASONING_EFFORT ?? cfg.observerRichReasoningEffort;
	cfg.observerRichReasoningSummary =
		process.env.CODEMEM_OBSERVER_RICH_REASONING_SUMMARY ?? cfg.observerRichReasoningSummary;
	if (process.env.CODEMEM_OBSERVER_RICH_MAX_OUTPUT_TOKENS != null) {
		const n = Number(process.env.CODEMEM_OBSERVER_RICH_MAX_OUTPUT_TOKENS);
		cfg.observerRichMaxOutputTokens = Number.isFinite(n) ? n : cfg.observerRichMaxOutputTokens;
	}
	if (process.env.CODEMEM_OBSERVER_OPENAI_USE_RESPONSES != null) {
		cfg.observerOpenAIUseResponses =
			process.env.CODEMEM_OBSERVER_OPENAI_USE_RESPONSES === "1" ||
			process.env.CODEMEM_OBSERVER_OPENAI_USE_RESPONSES === "true";
	}
	cfg.observerReasoningEffort =
		process.env.CODEMEM_OBSERVER_REASONING_EFFORT ?? cfg.observerReasoningEffort;
	cfg.observerReasoningSummary =
		process.env.CODEMEM_OBSERVER_REASONING_SUMMARY ?? cfg.observerReasoningSummary;
	if (process.env.CODEMEM_OBSERVER_MAX_OUTPUT_TOKENS != null) {
		const n = Number(process.env.CODEMEM_OBSERVER_MAX_OUTPUT_TOKENS);
		cfg.observerMaxOutputTokens = Number.isFinite(n) ? n : cfg.observerMaxOutputTokens;
	}
	cfg.observerAuthSource = process.env.CODEMEM_OBSERVER_AUTH_SOURCE ?? cfg.observerAuthSource;
	cfg.observerAuthFile = process.env.CODEMEM_OBSERVER_AUTH_FILE ?? cfg.observerAuthFile;
	cfg.observerMaxChars = parseIntSafe(process.env.CODEMEM_OBSERVER_MAX_CHARS, cfg.observerMaxChars);
	cfg.observerMaxTokens = parseIntSafe(
		process.env.CODEMEM_OBSERVER_MAX_TOKENS,
		cfg.observerMaxTokens,
	);
	cfg.observerAuthCacheTtlS = parseIntSafe(
		process.env.CODEMEM_OBSERVER_AUTH_CACHE_TTL_S,
		cfg.observerAuthCacheTtlS,
	);

	const envHeaders = coerceStringMap(process.env.CODEMEM_OBSERVER_HEADERS);
	if (envHeaders) cfg.observerHeaders = envHeaders;

	cfg.observerExplicitConfigKeys = collectExplicitObserverConfigKeys(data, process.env);

	return cfg;
}

// ---------------------------------------------------------------------------
// Auth error
// ---------------------------------------------------------------------------

export class ObserverAuthError extends Error {
	readonly code = "provider_auth_failed" as const;

	constructor(message: string) {
		super(message);
		this.name = "ObserverAuthError";
	}
}

function isAuthStatus(status: number): boolean {
	return status === 401 || status === 403;
}

// ---------------------------------------------------------------------------
// Anthropic helpers
// ---------------------------------------------------------------------------

function normalizeAnthropicModel(model: string): string {
	const normalized = model.trim();
	if (!normalized) return normalized;
	return ANTHROPIC_MODEL_ALIASES[normalized.toLowerCase()] ?? normalized;
}

function resolveAnthropicEndpoint(): string {
	return process.env.CODEMEM_ANTHROPIC_ENDPOINT ?? ANTHROPIC_MESSAGES_ENDPOINT;
}

function buildAnthropicHeaders(token: string): Record<string, string> {
	return {
		"anthropic-version": ANTHROPIC_VERSION,
		"content-type": "application/json",
		"x-api-key": token,
	};
}

function buildAnthropicPayload(
	model: string,
	systemPrompt: string,
	userPrompt: string,
	maxTokens: number,
	temperature: number | null,
	normalizeModel = true,
): Record<string, unknown> {
	const payload: Record<string, unknown> = {
		model: normalizeModel ? normalizeAnthropicModel(model) : model,
		max_tokens: maxTokens,
		system: systemPrompt,
		messages: [{ role: "user", content: userPrompt }],
	};
	if (temperature !== null) payload.temperature = temperature;
	return payload;
}

function buildAnthropicStructuredPayload(
	model: string,
	systemPrompt: string,
	userPrompt: string,
	maxTokens: number,
	schema: Record<string, unknown>,
	normalizeModel = true,
): Record<string, unknown> {
	return {
		model: normalizeModel ? normalizeAnthropicModel(model) : model,
		max_tokens: maxTokens,
		system: systemPrompt,
		messages: [{ role: "user", content: userPrompt }],
		output_config: {
			format: {
				type: "json_schema",
				schema,
			},
		},
	};
}

function parseAnthropicResponse(body: Record<string, unknown>): string | null {
	const content = body.content;
	if (!Array.isArray(content)) return null;
	const parts: string[] = [];
	for (const block of content) {
		if (
			typeof block === "object" &&
			block != null &&
			(block as Record<string, unknown>).type === "text"
		) {
			const text = (block as Record<string, unknown>).text;
			if (typeof text === "string") parts.push(text);
		}
	}
	return parts.length > 0 ? parts.join("") : null;
}

interface ObserverCallResult {
	raw: string | null;
	usage: ObserverTokenUsage | null;
}

function tokenCount(value: unknown): number | null {
	return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

function normalizeObserverUsage(body: Record<string, unknown>): ObserverTokenUsage | null {
	const usage = body.usage;
	if (typeof usage !== "object" || usage == null || Array.isArray(usage)) return null;
	const record = usage as Record<string, unknown>;
	const inputTokens = tokenCount(record.input_tokens) ?? tokenCount(record.prompt_tokens);
	const outputTokens = tokenCount(record.output_tokens) ?? tokenCount(record.completion_tokens);
	if (inputTokens == null || outputTokens == null) return null;

	const normalized: ObserverTokenUsage = { inputTokens, outputTokens };
	const totalTokens = tokenCount(record.total_tokens);
	if (totalTokens != null) normalized.totalTokens = totalTokens;

	const inputDetails = record.input_tokens_details ?? record.prompt_tokens_details;
	const cachedTokens =
		typeof inputDetails === "object" && inputDetails != null && !Array.isArray(inputDetails)
			? tokenCount((inputDetails as Record<string, unknown>).cached_tokens)
			: null;
	const cacheReadInputTokens = tokenCount(record.cache_read_input_tokens) ?? cachedTokens;
	if (cacheReadInputTokens != null) normalized.cacheReadInputTokens = cacheReadInputTokens;
	const cacheCreationInputTokens = tokenCount(record.cache_creation_input_tokens);
	if (cacheCreationInputTokens != null) {
		normalized.cacheCreationInputTokens = cacheCreationInputTokens;
	}
	return normalized;
}

function emptyCallResult(raw: string | null): ObserverCallResult {
	return { raw, usage: null };
}

function observerDiagnosticForCode(code: string): {
	reason: ObserverDiagnosticReason;
	nextAction: ObserverDiagnosticNextAction;
} {
	switch (code) {
		case "auth_missing":
		case "auth_failed":
			return { reason: "provider_auth_failed", nextAction: "configure_credential" };
		case "response_too_large":
			return { reason: "output_limit_exceeded", nextAction: "confirm_retry" };
		case "redirect_rejected":
			return { reason: "provider_redirect_rejected", nextAction: "activate_valid_manifest" };
		case "tls_rejected":
			return { reason: "provider_tls_rejected", nextAction: "activate_valid_manifest" };
		case "empty_response":
		case "invalid_model_id":
			return { reason: "output_invalid", nextAction: "activate_valid_manifest" };
		default:
			return { reason: "provider_unavailable", nextAction: "confirm_retry" };
	}
}

// ---------------------------------------------------------------------------
// OpenAI helpers
// ---------------------------------------------------------------------------

function buildOpenAIHeaders(token: string | null): Record<string, string> {
	return token
		? {
				authorization: `Bearer ${token}`,
				"content-type": "application/json",
			}
		: { "content-type": "application/json" };
}

function mergeHeadersCaseInsensitive(
	base: Record<string, string>,
	override: Record<string, string>,
): Record<string, string> {
	const merged: Record<string, string> = { ...base };
	for (const [key, value] of Object.entries(override)) {
		const normalizedKey = key.toLowerCase();
		for (const existingKey of Object.keys(merged)) {
			if (existingKey.toLowerCase() === normalizedKey) {
				delete merged[existingKey];
			}
		}
		merged[key] = value;
	}
	return merged;
}

function buildOpenAIPayload(
	model: string,
	systemPrompt: string,
	userPrompt: string,
	maxTokens: number,
	temperature: number | null,
): Record<string, unknown> {
	const payload: Record<string, unknown> = {
		model,
		max_tokens: maxTokens,
		messages: [
			{ role: "system", content: systemPrompt },
			{ role: "user", content: userPrompt },
		],
	};
	if (typeof temperature === "number" && Number.isFinite(temperature)) {
		payload.temperature = temperature;
	}
	return payload;
}

function buildOpenAIResponsesPayload(
	model: string,
	systemPrompt: string,
	userPrompt: string,
	maxOutputTokens: number,
	reasoningEffort: string | null,
	reasoningSummary: string | null,
	temperature: number | null,
): Record<string, unknown> {
	const payload: Record<string, unknown> = {
		model,
		max_output_tokens: maxOutputTokens,
		input: [
			{
				role: "developer",
				content: [{ type: "input_text", text: systemPrompt }],
			},
			{
				role: "user",
				content: [{ type: "input_text", text: userPrompt }],
			},
		],
	};
	if (typeof temperature === "number" && Number.isFinite(temperature)) {
		payload.temperature = temperature;
	}
	if (reasoningEffort || reasoningSummary) {
		const reasoning: Record<string, unknown> = {};
		if (reasoningEffort) reasoning.effort = reasoningEffort;
		if (reasoningSummary) reasoning.summary = reasoningSummary;
		payload.reasoning = reasoning;
	}
	return payload;
}

function buildOpenAIResponsesStructuredPayload(
	model: string,
	systemPrompt: string,
	userPrompt: string,
	maxOutputTokens: number,
	reasoningEffort: string | null,
	reasoningSummary: string | null,
	temperature: number | null,
	schemaName: string,
	schema: Record<string, unknown>,
): Record<string, unknown> {
	const payload = buildOpenAIResponsesPayload(
		model,
		systemPrompt,
		userPrompt,
		maxOutputTokens,
		reasoningEffort,
		reasoningSummary,
		temperature,
	);
	payload.text = {
		format: {
			type: "json_schema",
			name: schemaName,
			schema,
			strict: true,
		},
	};
	return payload;
}

function parseOpenAIResponse(body: Record<string, unknown>): string | null {
	const choices = body.choices;
	if (!Array.isArray(choices) || choices.length === 0) return null;
	const first = choices[0] as Record<string, unknown> | undefined;
	if (!first) return null;
	const message = first.message as Record<string, unknown> | undefined;
	if (!message) return null;
	const content = message.content;
	if (typeof content === "string") return content;
	if (!Array.isArray(content)) return null;
	const parts: string[] = [];
	for (const block of content) {
		if (typeof block !== "object" || block == null) continue;
		const record = block as Record<string, unknown>;
		if (
			(record.type === "text" || record.type === "output_text") &&
			typeof record.text === "string"
		) {
			parts.push(record.text);
		}
	}
	return parts.length > 0 ? parts.join("") : null;
}

function parseOpenAIResponsesResponse(body: Record<string, unknown>): string | null {
	const outputText = body.output_text;
	if (typeof outputText === "string" && outputText.trim()) return outputText;
	const output = body.output;
	if (!Array.isArray(output)) return null;
	const parts: string[] = [];
	for (const item of output) {
		if (typeof item !== "object" || item == null) continue;
		const content = (item as Record<string, unknown>).content;
		if (!Array.isArray(content)) continue;
		for (const block of content) {
			if (typeof block !== "object" || block == null) continue;
			const record = block as Record<string, unknown>;
			if (record.type === "output_text" && typeof record.text === "string") {
				parts.push(record.text);
			}
		}
	}
	return parts.length > 0 ? parts.join("") : null;
}

// ---------------------------------------------------------------------------
// nowMs helper
// ---------------------------------------------------------------------------

function nowMs(): number {
	return Date.now();
}

async function readBoundedResponseBody(
	response: Response,
	maxBytes: number,
): Promise<Uint8Array | null> {
	const declaredLength = Number(response.headers.get("content-length"));
	if (Number.isFinite(declaredLength) && declaredLength > maxBytes) {
		await response.body?.cancel().catch(() => undefined);
		return null;
	}
	if (!response.body) return new Uint8Array();
	const reader = response.body.getReader();
	const chunks: Uint8Array[] = [];
	let total = 0;
	try {
		for (;;) {
			const next = await reader.read();
			if (next.done) break;
			total += next.value.byteLength;
			if (total > maxBytes) {
				await reader.cancel().catch(() => undefined);
				return null;
			}
			chunks.push(next.value);
		}
	} finally {
		reader.releaseLock();
	}
	const output = new Uint8Array(total);
	let offset = 0;
	for (const chunk of chunks) {
		output.set(chunk, offset);
		offset += chunk.byteLength;
	}
	return output;
}

function isManifestProviderChoice(
	value: ObserverConfig | ProviderChoiceV1 | undefined,
): value is ProviderChoiceV1 {
	return Boolean(
		value && "providerFingerprint" in value && "wireProtocol" in value && "endpointUrl" in value,
	);
}

const LEGACY_OBSERVER_CONSTRUCTION = Symbol("legacy-observer-construction");

export type ObserverHealthOptions = {
	onHealthState?: (state: "healthy" | "unhealthy") => void | Promise<void>;
	destinationBoundary?: DestinationBoundaryV1;
};

// ---------------------------------------------------------------------------
// ObserverClient
// ---------------------------------------------------------------------------

/**
 * LLM client for analyzing coding session transcripts and extracting memories.
 *
 * Resolves provider + auth from codemem config, then calls the LLM via fetch.
 * Supports Anthropic Messages and OpenAI-compatible APIs.
 */
export class ObserverClient {
	readonly provider: string;
	readonly requestedModel: string | null;
	model: string;
	readonly runtime: string;
	readonly temperature: number | null;
	readonly tierRoutingEnabled: boolean;
	readonly simpleProvider: string | null;
	readonly simpleModel: string | null;
	readonly simpleTemperature: number | null;
	readonly richProvider: string | null;
	readonly richModel: string | null;
	readonly richTemperature: number | null;
	readonly richReasoningEffort: string | null;
	readonly richReasoningSummary: string | null;
	readonly richMaxOutputTokens: number | null;
	readonly openaiUseResponses: boolean;
	readonly reasoningEffort: string | null;
	readonly reasoningSummary: string | null;
	readonly maxChars: number;
	readonly maxTokens: number;
	readonly maxOutputTokens: number;
	readonly authSource: string;
	readonly authFile: string | null;
	readonly authCacheTtlS: number;

	/** Resolved auth material — updated on refresh. */
	auth: ObserverAuthMaterial;
	readonly authAdapter: ObserverAuthAdapter;

	private _observerHeaders: Record<string, string>;
	private _customBaseUrl: string | null;
	private _customBaseUrlAllowsNoAuth: boolean;
	private readonly _apiKey: string | null;
	private readonly _manifestChoice: ProviderChoiceV1 | null;
	private readonly _maxResponseBytes: number;
	private readonly _requestTimeoutMs: number;
	private readonly _onHealthState: ObserverHealthOptions["onHealthState"];
	private readonly _destinationBoundary: DestinationBoundaryV1 | null;

	// Error tracking
	private _lastErrorCode: string | null = null;
	private _lastErrorReason: ObserverDiagnosticReason | null = null;
	private _lastErrorNextAction: ObserverDiagnosticNextAction | null = null;
	private readonly _observerExplicitConfigKeys: string[];

	constructor(
		provider: ProviderChoiceV1,
		profile: ObserverTransportProfile,
		options?: ObserverHealthOptions,
	);
	constructor(
		config: ObserverConfig,
		profile: undefined,
		legacyToken: typeof LEGACY_OBSERVER_CONSTRUCTION,
	);
	constructor(
		configOrProvider?: ObserverConfig | ProviderChoiceV1,
		manifestProfile?: ObserverTransportProfile,
		legacyToken?: typeof LEGACY_OBSERVER_CONSTRUCTION | ObserverHealthOptions,
	) {
		if (isManifestProviderChoice(configOrProvider)) {
			if (!manifestProfile) throw new Error("A frozen Observer transport profile is required.");
			const provider = validateProviderChoice(configOrProvider);
			const profile = validateProviderTransportProfile(manifestProfile);
			this._manifestChoice = provider;
			this._maxResponseBytes = profile.observerMaxResponseBytes;
			this._requestTimeoutMs = profile.observerRequestTimeoutMs;
			const options = legacyToken as ObserverHealthOptions | undefined;
			this._onHealthState = options?.onHealthState;
			this._destinationBoundary = options?.destinationBoundary ?? null;
			if (this._destinationBoundary) destinationBoundaryFingerprint(this._destinationBoundary);
			this.provider = provider.wireProtocol === "anthropic_messages_v1" ? "anthropic" : "openai";
			this.requestedModel = provider.modelId;
			this.model = provider.modelId;
			this.runtime = "api_http";
			this.temperature = profile.observerTemperature;
			this.tierRoutingEnabled = false;
			this.simpleProvider = null;
			this.simpleModel = null;
			this.simpleTemperature = null;
			this.richProvider = null;
			this.richModel = null;
			this.richTemperature = null;
			this.richReasoningEffort = null;
			this.richReasoningSummary = null;
			this.richMaxOutputTokens = null;
			this.openaiUseResponses = false;
			this.reasoningEffort = null;
			this.reasoningSummary = null;
			this.maxChars = profile.observerMaxInputChars;
			this.maxTokens = profile.observerMaxOutputTokens;
			this.maxOutputTokens = profile.observerMaxOutputTokens;
			this.authSource =
				provider.credentialRef.kind === "environment"
					? `environment:${provider.credentialRef.name}`
					: "none";
			this.authFile = null;
			this.authCacheTtlS = 0;
			this._observerHeaders = {};
			this._customBaseUrl = provider.endpointUrl;
			this._customBaseUrlAllowsNoAuth = provider.credentialRef.kind === "none";
			this._apiKey = null;
			this._observerExplicitConfigKeys = [];
			this.authAdapter = new ObserverAuthAdapter({ source: "none", cacheTtlS: 0 });
			this.auth = { token: null, authType: "none", source: "none" };
			this._initProvider(false);
			return;
		}

		if (legacyToken !== LEGACY_OBSERVER_CONSTRUCTION) {
			throw new Error("ObserverClient requires a validated frozen provider choice and profile.");
		}
		this._manifestChoice = null;
		this._onHealthState = undefined;
		this._destinationBoundary = null;
		this._maxResponseBytes = Number.POSITIVE_INFINITY;
		this._requestTimeoutMs = FETCH_TIMEOUT_MS;
		const config = configOrProvider;
		const configWasProvided = config !== undefined;
		const cfg = config ?? loadObserverConfig();
		const explicitConfigKeys = resolveExplicitObserverConfigKeys(cfg, configWasProvided);
		this._observerExplicitConfigKeys = [...explicitConfigKeys];

		const provider = (cfg.observerProvider ?? "").toLowerCase();
		const model = (cfg.observerModel ?? "").trim();
		this.requestedModel = model || null;

		// Collect known custom providers
		const customProviders = listConfiguredOpenCodeProviders();
		if (provider && provider !== "openai" && provider !== "anthropic") {
			customProviders.add(provider);
		}

		// Resolve provider
		let resolved = provider;
		if (!resolved) {
			const inferred = resolveCustomProviderFromModel(model, customProviders);
			if (inferred) resolved = inferred;
		}
		if (!resolved) {
			const builtIn = resolveBuiltInProviderFromModel(model);
			if (builtIn) resolved = builtIn;
		}
		if (!resolved) {
			resolved = model.toLowerCase().startsWith("claude") ? "anthropic" : "openai";
		}
		if (
			resolved !== "openai" &&
			resolved !== "anthropic" &&
			resolved !== "opencode" &&
			!customProviders.has(resolved)
		) {
			resolved = "openai";
		}
		this.provider = resolved;

		this.runtime = "api_http";

		// Resolve model
		if (model) {
			this.model = model;
		} else if (resolved === "anthropic") {
			this.model = DEFAULT_ANTHROPIC_MODEL;
		} else if (resolved === "openai") {
			this.model = DEFAULT_OPENAI_MODEL;
		} else {
			this.model =
				resolveBuiltInProviderDefaultModel(resolved) ??
				resolveCustomProviderDefaultModel(resolved) ??
				"";
		}

		this.temperature =
			typeof cfg.observerTemperature === "number" && Number.isFinite(cfg.observerTemperature)
				? cfg.observerTemperature
				: 0.2;
		const hasCustomBaseUrl =
			typeof cfg.observerBaseUrl === "string" && cfg.observerBaseUrl.trim().length > 0;
		this.tierRoutingEnabled = explicitConfigKeys.has("observerTierRoutingEnabled")
			? cfg.observerTierRoutingEnabled === true
			: supportsDefaultTierRouting(this.provider, this.runtime, hasCustomBaseUrl);
		this.simpleProvider =
			typeof cfg.observerSimpleProvider === "string" && cfg.observerSimpleProvider.trim()
				? cfg.observerSimpleProvider.trim()
				: null;
		this.simpleModel =
			typeof cfg.observerSimpleModel === "string" && cfg.observerSimpleModel.trim()
				? cfg.observerSimpleModel.trim()
				: null;
		this.simpleTemperature =
			typeof cfg.observerSimpleTemperature === "number" &&
			Number.isFinite(cfg.observerSimpleTemperature)
				? cfg.observerSimpleTemperature
				: null;
		this.richProvider =
			typeof cfg.observerRichProvider === "string" && cfg.observerRichProvider.trim()
				? cfg.observerRichProvider.trim()
				: null;
		this.richModel =
			typeof cfg.observerRichModel === "string" && cfg.observerRichModel.trim()
				? cfg.observerRichModel.trim()
				: null;
		this.richTemperature =
			typeof cfg.observerRichTemperature === "number" &&
			Number.isFinite(cfg.observerRichTemperature)
				? cfg.observerRichTemperature
				: null;
		this.richReasoningEffort =
			typeof cfg.observerRichReasoningEffort === "string" && cfg.observerRichReasoningEffort.trim()
				? cfg.observerRichReasoningEffort.trim()
				: null;
		this.richReasoningSummary =
			typeof cfg.observerRichReasoningSummary === "string" &&
			cfg.observerRichReasoningSummary.trim()
				? cfg.observerRichReasoningSummary.trim()
				: null;
		this.richMaxOutputTokens =
			typeof cfg.observerRichMaxOutputTokens === "number" &&
			Number.isFinite(cfg.observerRichMaxOutputTokens)
				? cfg.observerRichMaxOutputTokens
				: null;
		const configuredOpenAIUseResponses = explicitConfigKeys.has("observerOpenAIUseResponses")
			? cfg.observerOpenAIUseResponses === true
			: this.provider === "openai" && this.runtime === "api_http";
		this.openaiUseResponses =
			this.provider === "openai" && this.runtime === "api_http" && !hasCustomBaseUrl
				? true
				: configuredOpenAIUseResponses;
		const usesCustomOpenAIChatCompletions =
			this.provider === "openai" &&
			this.runtime === "api_http" &&
			hasCustomBaseUrl &&
			!this.openaiUseResponses;
		this.reasoningEffort =
			!usesCustomOpenAIChatCompletions &&
			typeof cfg.observerReasoningEffort === "string" &&
			cfg.observerReasoningEffort.trim()
				? cfg.observerReasoningEffort.trim()
				: null;
		this.reasoningSummary =
			!usesCustomOpenAIChatCompletions &&
			typeof cfg.observerReasoningSummary === "string" &&
			cfg.observerReasoningSummary.trim()
				? cfg.observerReasoningSummary.trim()
				: null;
		const usesActiveOpenAIReasoning =
			this.openaiUseResponses &&
			((this.reasoningEffort != null && this.reasoningEffort.toLowerCase() !== "none") ||
				this.reasoningSummary != null);
		// GPT-5.1+ Responses requests only accept sampling controls when
		// reasoning effort is `none`; keep the effective config aligned with
		// what the request can actually transmit.
		if (usesActiveOpenAIReasoning) this.temperature = null;
		this.maxChars = cfg.observerMaxChars;
		this.maxTokens = cfg.observerMaxTokens;
		this.maxOutputTokens =
			typeof cfg.observerMaxOutputTokens === "number" &&
			Number.isFinite(cfg.observerMaxOutputTokens)
				? cfg.observerMaxOutputTokens
				: this.maxTokens;
		this.authSource = cfg.observerAuthSource;
		this.authFile = cfg.observerAuthFile;
		this.authCacheTtlS = cfg.observerAuthCacheTtlS;
		this._observerHeaders = { ...cfg.observerHeaders };
		this._apiKey = cfg.observerApiKey ?? null;

		const baseUrl = cfg.observerBaseUrl;
		this._customBaseUrl = typeof baseUrl === "string" && baseUrl.trim() ? baseUrl.trim() : null;
		this._customBaseUrlAllowsNoAuth = this._customBaseUrl != null;

		// Set up auth adapter
		this.authAdapter = new ObserverAuthAdapter({
			source: cfg.observerAuthSource,
			filePath: cfg.observerAuthFile,
			cacheTtlS: Math.max(0, cfg.observerAuthCacheTtlS),
		});
		this.auth = { token: null, authType: "none", source: "none" };

		this._initProvider(false);
	}

	toConfig(): ObserverConfig {
		return {
			observerProvider: this.provider,
			observerModel: this.model,
			observerRuntime: this.runtime,
			observerApiKey: this._apiKey,
			observerBaseUrl: this._customBaseUrl,
			observerTemperature: this.temperature,
			observerTierRoutingEnabled: this.tierRoutingEnabled,
			observerSimpleProvider: this.simpleProvider,
			observerSimpleModel: this.simpleModel,
			observerSimpleTemperature: this.simpleTemperature,
			observerRichProvider: this.richProvider,
			observerRichModel: this.richModel,
			observerRichTemperature: this.richTemperature,
			observerRichReasoningEffort: this.richReasoningEffort,
			observerRichReasoningSummary: this.richReasoningSummary,
			observerRichMaxOutputTokens: this.richMaxOutputTokens,
			observerOpenAIUseResponses: this.openaiUseResponses,
			observerReasoningEffort: this.reasoningEffort,
			observerReasoningSummary: this.reasoningSummary,
			observerMaxOutputTokens: this.maxOutputTokens,
			observerMaxChars: this.maxChars,
			observerMaxTokens: this.maxTokens,
			observerHeaders: { ...this._observerHeaders },
			observerAuthSource: this.authSource,
			observerAuthFile: this.authFile,
			observerAuthCacheTtlS: this.authCacheTtlS,
			observerExplicitConfigKeys: [...this._observerExplicitConfigKeys],
		};
	}

	/** Return the resolved runtime state of this observer client. */
	getStatus(): ObserverStatus {
		let method = "none";
		if (this.provider === "opencode" && this.auth.token) {
			method = "sdk_client";
		} else if (this.auth.token) {
			method = "api_direct";
		}

		const runtime =
			this.provider === "openai" && this.openaiUseResponses && method === "api_direct"
				? "responses_api"
				: this.runtime;

		const status: ObserverStatus = {
			provider: this.provider,
			model: this.model,
			runtime,
			auth: {
				source: this.auth.source,
				type: method,
				hasToken: !!this.auth.token,
			},
		};
		if (this._lastErrorReason && this._lastErrorNextAction) {
			const boundary = this._destinationBoundary;
			const diagnostic: NonNullable<ObserverStatus["lastError"]> = {
				version: 1,
				action: "failed",
				reason: this._lastErrorReason,
				destination:
					boundary?.executionLocation ?? this._manifestChoice?.executionLocation ?? "unknown",
				counts: {
					considered: 0,
					transmitted: 0,
					eligible: 0,
					localOnly: 0,
					private: 0,
					secret: 0,
				},
				configurationFingerprint: boundary?.configurationFingerprint ?? null,
				providerFingerprint:
					boundary?.providerFingerprint ?? this._manifestChoice?.providerFingerprint ?? null,
				nextAction: this._lastErrorNextAction,
			};
			Object.defineProperty(diagnostic, "code", {
				value: this._lastErrorCode ?? "observer_error",
				enumerable: false,
			});
			status.lastError = diagnostic;
		}
		return status;
	}

	/** Force-refresh auth credentials. */
	refreshAuth(force = true): void {
		this.authAdapter.invalidateCache();
		this._initProvider(force);
	}

	private canCallOpenAIDirectWithoutAuth(): boolean {
		if (this._manifestChoice) return this._manifestChoice.credentialRef.kind === "none";
		return this._customBaseUrlAllowsNoAuth && this.provider !== "anthropic";
	}

	/**
	 * Call the LLM with a system prompt and user prompt, return the response.
	 *
	 * This is the main entry point. On auth errors, attempts one refresh + retry.
	 */
	async observe(systemPrompt: string, userPrompt: string): Promise<ObserverResponse> {
		const startedAt = nowMs();
		// Enforce configured prompt-length cap (matches Python behavior)
		const maxChars = this.maxChars;
		const minUserBudget = Math.floor(maxChars * 0.25);
		const systemBudget = Math.max(0, maxChars - minUserBudget);
		const clippedSystem = (
			systemPrompt.length > systemBudget ? systemPrompt.slice(0, systemBudget) : systemPrompt
		).toWellFormed();
		const userBudget = Math.max(minUserBudget, maxChars - clippedSystem.length);
		const clippedUser = (
			userPrompt.length > userBudget ? userPrompt.slice(0, userBudget) : userPrompt
		).toWellFormed();

		try {
			return await this._observeOnce(clippedSystem, clippedUser, startedAt);
		} catch (err) {
			if (err instanceof ObserverAuthError) {
				try {
					// Attempt one auth refresh + retry before reporting the logical outcome.
					this.refreshAuth();
					if (!this.auth.token) throw err;
					return await this._observeOnce(clippedSystem, clippedUser, startedAt);
				} catch {
					await this._reportHealth("unhealthy");
					throw err; // re-throw original
				}
			}
			await this._reportHealth("unhealthy");
			throw err;
		}
	}

	private async _observeOnce(
		systemPrompt: string,
		userPrompt: string,
		startedAt: number,
	): Promise<ObserverResponse> {
		const call = await this._callOnce(systemPrompt, userPrompt);
		if (call.raw) this._clearLastError();
		await this._reportHealth(call.raw ? "healthy" : "unhealthy");
		return this._buildResponse(call, startedAt);
	}

	private async _reportHealth(state: "healthy" | "unhealthy"): Promise<void> {
		try {
			await this._onHealthState?.(state);
		} catch {
			console.error("[codemem] observer health callback failed; inference result was preserved.");
		}
	}

	private _buildResponse(call: ObserverCallResult, startedAt: number): ObserverResponse {
		return {
			raw: call.raw,
			parsed: call.raw ? tryParseJSON(call.raw) : null,
			provider: this.provider,
			model: this.model,
			elapsedMs: Math.max(0, nowMs() - startedAt),
			usage: call.usage,
		};
	}

	/**
	 * Request structured JSON output when the current provider/runtime supports it.
	 * Falls back to plain `observe()` + caller-side parsing for unsupported paths.
	 */
	async observeStructuredJson(
		systemPrompt: string,
		userPrompt: string,
		schemaName: string,
		schema: Record<string, unknown>,
	): Promise<ObserverStructuredJsonResponse> {
		const startedAt = nowMs();
		if (this._manifestChoice) {
			const fallback = await this.observe(systemPrompt, userPrompt);
			return {
				...fallback,
				elapsedMs: Math.max(0, nowMs() - startedAt),
				usedStructuredOutputs: false,
			};
		}
		if (this.provider === "openai" && this.openaiUseResponses) {
			if (!this.auth.token && !this.canCallOpenAIDirectWithoutAuth()) {
				this._initProvider(true);
				if (!this.auth.token && !this.canCallOpenAIDirectWithoutAuth()) {
					return {
						raw: null,
						parsed: null,
						provider: this.provider,
						model: this.model,
						elapsedMs: Math.max(0, nowMs() - startedAt),
						usage: null,
						usedStructuredOutputs: true,
					};
				}
			}

			let url: string;
			if (this._customBaseUrl) {
				url = `${stripTrailingSlashes(this._customBaseUrl)}/responses`;
			} else {
				url = "https://api.openai.com/v1/responses";
			}
			const headers = buildOpenAIHeaders(this.auth.token);
			const mergedHeaders = mergeHeadersCaseInsensitive(
				headers,
				renderObserverHeaders(this._observerHeaders, this.auth),
			);
			const payload = buildOpenAIResponsesStructuredPayload(
				this.model,
				systemPrompt,
				userPrompt,
				this.maxOutputTokens,
				this.reasoningEffort,
				this.reasoningSummary,
				this.temperature,
				schemaName,
				schema,
			);
			const call = await this._fetchJSON(url, mergedHeaders, payload, {
				parseResponse: parseOpenAIResponsesResponse,
				providerLabel: "OpenAI",
			});
			return {
				raw: call.raw,
				parsed: call.raw ? tryParseJSON(call.raw) : null,
				provider: this.provider,
				model: this.model,
				elapsedMs: Math.max(0, nowMs() - startedAt),
				usage: call.usage,
				usedStructuredOutputs: true,
			};
		}

		if (this.provider === "anthropic") {
			if (!this.auth.token) {
				this._initProvider(true);
			}
			if (this.auth.token) {
				const headers = buildAnthropicHeaders(this.auth.token);
				const mergedHeaders = mergeHeadersCaseInsensitive(
					headers,
					renderObserverHeaders(this._observerHeaders, this.auth),
				);
				const call = await this._fetchJSON(
					resolveAnthropicEndpoint(),
					mergedHeaders,
					buildAnthropicStructuredPayload(
						this.model,
						systemPrompt,
						userPrompt,
						this.maxTokens,
						schema,
						this._manifestChoice === null,
					),
					{ parseResponse: parseAnthropicResponse, providerLabel: "Anthropic" },
				);
				return {
					raw: call.raw,
					parsed: call.raw ? tryParseJSON(call.raw) : null,
					provider: this.provider,
					model: this.model,
					elapsedMs: Math.max(0, nowMs() - startedAt),
					usage: call.usage,
					usedStructuredOutputs: true,
				};
			}
			// No token — fall through to observe() so it reports the standard auth error.
		}

		const fallback = await this.observe(systemPrompt, userPrompt);
		return {
			raw: fallback.raw,
			parsed: fallback.parsed,
			provider: fallback.provider,
			model: fallback.model,
			elapsedMs: Math.max(0, nowMs() - startedAt),
			usage: fallback.usage,
			usedStructuredOutputs: false,
		};
	}

	// -----------------------------------------------------------------------
	// Provider initialization
	// -----------------------------------------------------------------------

	private _initProvider(forceRefresh: boolean): void {
		if (this._manifestChoice) {
			const credential = this._manifestChoice.credentialRef;
			if (credential.kind === "environment") {
				const token = process.env[credential.name]?.trim() || null;
				this.auth = token
					? { token, authType: "api_key", source: `environment:${credential.name}` }
					: { token: null, authType: "none", source: credential.kind };
			} else {
				this.auth = { token: null, authType: "none", source: credential.kind };
			}
			return;
		}
		if (this.provider !== "openai" && this.provider !== "anthropic") {
			// Custom provider — resolve base URL, model ID, and headers from OpenCode config
			const providerConfig = getOpenCodeProviderConfig(this.provider);
			const hasExplicitProviderConfig = Object.keys(providerConfig).length > 0;
			const [baseUrl, modelId, providerHeaders] = hasExplicitProviderConfig
				? resolveCustomProviderModel(this.provider, this.model)
				: resolveBuiltInProviderModel(this.provider, this.model);

			// Persist resolved values for use in _callOpenAIDirect
			if (baseUrl && !this._customBaseUrl) {
				this._customBaseUrl = baseUrl;
				this._customBaseUrlAllowsNoAuth = hasExplicitProviderConfig;
			}
			if (modelId) this.model = modelId;
			if (providerHeaders && Object.keys(providerHeaders).length > 0) {
				this._observerHeaders = { ...this._observerHeaders, ...providerHeaders };
			}

			const effectiveBaseUrl = this._customBaseUrl;
			if (!effectiveBaseUrl) return;

			const apiKey = getProviderApiKey(providerConfig) || this._apiKey;

			this.auth = this.authAdapter.resolve({
				explicitToken: apiKey,
				envTokens: [process.env.CODEMEM_OBSERVER_API_KEY ?? ""],
				forceRefresh,
			});
		} else if (this.provider === "anthropic") {
			this.auth = this.authAdapter.resolve({
				explicitToken: this._apiKey,
				envTokens: [process.env.ANTHROPIC_API_KEY ?? ""],
				forceRefresh,
			});
		} else {
			// OpenAI
			this.auth = this.authAdapter.resolve({
				explicitToken: this._apiKey,
				envTokens: [
					process.env.OPENCODE_API_KEY ?? "",
					process.env.OPENAI_API_KEY ?? "",
					process.env.CODEX_API_KEY ?? "",
				],
				forceRefresh,
			});
		}
	}

	// -----------------------------------------------------------------------
	// LLM call dispatch
	// -----------------------------------------------------------------------

	private async _callOnce(systemPrompt: string, userPrompt: string): Promise<ObserverCallResult> {
		// Refresh if we have no token
		if (!this.auth.token && !this.canCallOpenAIDirectWithoutAuth()) {
			this._initProvider(true);
			if (!this.auth.token && !this.canCallOpenAIDirectWithoutAuth()) {
				this._setLastError(`${capitalize(this.provider)} credentials are missing.`, "auth_missing");
				return emptyCallResult(null);
			}
		}

		// Direct API call via fetch
		if (this.provider === "anthropic") {
			return this._callAnthropicDirect(systemPrompt, userPrompt);
		}
		return this._callOpenAIDirect(systemPrompt, userPrompt);
	}

	// -----------------------------------------------------------------------
	// Anthropic direct (API key)
	// -----------------------------------------------------------------------

	private async _callAnthropicDirect(
		systemPrompt: string,
		userPrompt: string,
	): Promise<ObserverCallResult> {
		const url = this._manifestChoice?.endpointUrl ?? resolveAnthropicEndpoint();
		const token = this.auth.token ?? "";
		const headers =
			this._manifestChoice?.credentialRef.kind === "none"
				? { "anthropic-version": ANTHROPIC_VERSION, "content-type": "application/json" }
				: buildAnthropicHeaders(token);
		const mergedHeaders = mergeHeadersCaseInsensitive(
			headers,
			renderObserverHeaders(this._observerHeaders, this.auth),
		);
		const payload = buildAnthropicPayload(
			this.model,
			systemPrompt,
			userPrompt,
			this.maxTokens,
			this._manifestChoice ? this.temperature : null,
			this._manifestChoice === null,
		);

		return this._fetchJSON(url, mergedHeaders, payload, {
			parseResponse: parseAnthropicResponse,
			providerLabel: "Anthropic",
		});
	}

	// -----------------------------------------------------------------------
	// OpenAI direct (API key)
	// -----------------------------------------------------------------------

	private async _callOpenAIDirect(
		systemPrompt: string,
		userPrompt: string,
	): Promise<ObserverCallResult> {
		let url: string;
		if (this._manifestChoice) {
			url = this._manifestChoice.endpointUrl;
		} else if (this._customBaseUrl) {
			url = `${stripTrailingSlashes(this._customBaseUrl)}/${this.openaiUseResponses ? "responses" : "chat/completions"}`;
		} else {
			url = this.openaiUseResponses
				? "https://api.openai.com/v1/responses"
				: "https://api.openai.com/v1/chat/completions";
		}

		const headers = buildOpenAIHeaders(this.auth.token);
		const mergedHeaders = mergeHeadersCaseInsensitive(
			headers,
			renderObserverHeaders(this._observerHeaders, this.auth),
		);
		const payload = this.openaiUseResponses
			? buildOpenAIResponsesPayload(
					this.model,
					systemPrompt,
					userPrompt,
					this.maxOutputTokens,
					this.reasoningEffort,
					this.reasoningSummary,
					this.temperature,
				)
			: buildOpenAIPayload(this.model, systemPrompt, userPrompt, this.maxTokens, this.temperature);

		return this._fetchJSON(url, mergedHeaders, payload, {
			parseResponse: this.openaiUseResponses ? parseOpenAIResponsesResponse : parseOpenAIResponse,
			providerLabel: capitalize(this.provider),
		});
	}

	// -----------------------------------------------------------------------
	// Shared fetch: JSON response (non-streaming)
	// -----------------------------------------------------------------------

	private async _fetchJSON(
		url: string,
		headers: Record<string, string>,
		payload: Record<string, unknown>,
		opts: {
			parseResponse: (body: Record<string, unknown>) => string | null;
			providerLabel: string;
		},
	): Promise<ObserverCallResult> {
		try {
			const response = await fetch(url, {
				method: "POST",
				headers,
				body: JSON.stringify(payload),
				signal: AbortSignal.timeout(this._requestTimeoutMs),
				redirect: "manual",
			});
			const responseBytes = await readBoundedResponseBody(response, this._maxResponseBytes);
			if (responseBytes === null) {
				this._setLastError(
					"Provider response exceeded the frozen byte limit.",
					"response_too_large",
				);
				return emptyCallResult(null);
			}
			const responseText = new TextDecoder().decode(responseBytes);

			if (!response.ok) {
				this._handleHttpError(response.status, responseText, opts.providerLabel);
				return emptyCallResult(null);
			}
			const body = JSON.parse(responseText) as Record<string, unknown>;
			const result = opts.parseResponse(body);
			if (result === null) {
				this._setLastError(
					`${opts.providerLabel} returned 200 but response contained no extractable text.`,
					"empty_response",
				);
			}
			return { raw: result, usage: normalizeObserverUsage(body) };
		} catch (err) {
			if (err instanceof ObserverAuthError) throw err;
			this._setLastError(
				`${opts.providerLabel} processing failed during observer inference.`,
				"observer_call_failed",
			);
			return emptyCallResult(null);
		}
	}

	// -----------------------------------------------------------------------
	// Error handling
	// -----------------------------------------------------------------------

	private _handleHttpError(status: number, errorText: string, providerLabel: string): void {
		if (isAuthStatus(status)) {
			this._setLastError(
				`${providerLabel} authentication failed. Refresh credentials and retry.`,
				"auth_failed",
			);
			throw new ObserverAuthError("provider_auth_failed");
		}

		if (status === 429) {
			this._setLastError(`${providerLabel} rate limited. Retry later.`, "rate_limited");
			return;
		}
		if (status >= 300 && status < 400) {
			this._setLastError("provider_redirect_rejected", "redirect_rejected");
			return;
		}

		// Check for model-not-found in Anthropic error responses
		if (errorText) {
			try {
				const parsed = JSON.parse(errorText) as Record<string, unknown>;
				const error = parsed.error as Record<string, unknown> | undefined;
				if (error && typeof error === "object") {
					const errorType = String(error.type ?? "").toLowerCase();
					if (errorType === "not_found_error") {
						this._setLastError("output_invalid", "invalid_model_id");
						return;
					}
				}
			} catch {
				// not JSON — ignore
			}
		}

		this._setLastError(`${providerLabel} request failed (${status}).`, "provider_request_failed");
	}

	private _setLastError(_message: string, code?: string): void {
		const safeCode = (code ?? "observer_error").trim() || "observer_error";
		const diagnostic = observerDiagnosticForCode(safeCode);
		this._lastErrorCode = safeCode;
		this._lastErrorReason = diagnostic.reason;
		this._lastErrorNextAction = diagnostic.nextAction;
	}

	private _clearLastError(): void {
		this._lastErrorCode = null;
		this._lastErrorReason = null;
		this._lastErrorNextAction = null;
	}
}

/** Internal benchmark/setup-translation compatibility; intentionally absent from the package barrel. */
export function createLegacyObserverClient(
	config: ObserverConfig = loadObserverConfig(),
): ObserverClient {
	return new ObserverClient(config, undefined, LEGACY_OBSERVER_CONSTRUCTION);
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

function capitalize(s: string): string {
	return s.charAt(0).toUpperCase() + s.slice(1);
}

function tryParseJSON(text: string): Record<string, unknown> | null {
	try {
		const parsed = JSON.parse(text);
		return typeof parsed === "object" && parsed != null && !Array.isArray(parsed)
			? (parsed as Record<string, unknown>)
			: null;
	} catch {
		return null;
	}
}
