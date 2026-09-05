/**
 * Observer authentication: credential resolution, caching, and header rendering.
 *
 * Resolves auth tokens from explicit values, environment variables, or files. Supports
 * template-based header rendering with `${auth.token}` / `${auth.type}` / `${auth.source}`.
 */

import { existsSync, readFileSync, statSync } from "node:fs";
import { codememHomeDir } from "./home.js";

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

const REDACT_PATTERNS = [/sk-[A-Za-z0-9]{10,}/g, /Bearer\s+[A-Za-z0-9._-]{10,}/g];

/** Redact API keys and bearer tokens in text. Truncates at `limit` chars. */
export function redactText(text: string, limit = 400): string {
	let redacted = text;
	for (const pattern of REDACT_PATTERNS) {
		redacted = redacted.replace(new RegExp(pattern.source, pattern.flags), "[redacted]");
	}
	return redacted.length > limit ? `${redacted.slice(0, limit)}…` : redacted;
}

// ---------------------------------------------------------------------------
// Auth material
// ---------------------------------------------------------------------------

export interface ObserverAuthMaterial {
	token: string | null;
	authType: string;
	source: string;
}

function noAuth(): ObserverAuthMaterial {
	return { token: null, authType: "none", source: "none" };
}

// ---------------------------------------------------------------------------
// Credential availability
// ---------------------------------------------------------------------------

/** Probe which credential sources are currently available per built-in provider. */
export function probeAvailableCredentials(): Record<
	string,
	{ api_key: boolean; env_var: boolean }
> {
	const explicitApiKey = Boolean(process.env.CODEMEM_OBSERVER_API_KEY);
	return {
		openai: {
			api_key: explicitApiKey,
			env_var: Boolean(
				process.env.OPENCODE_API_KEY || process.env.OPENAI_API_KEY || process.env.CODEX_API_KEY,
			),
		},
		anthropic: { api_key: explicitApiKey, env_var: Boolean(process.env.ANTHROPIC_API_KEY) },
		opencode: { api_key: explicitApiKey, env_var: Boolean(process.env.OPENCODE_API_KEY) },
	};
}

// ---------------------------------------------------------------------------
// External auth: file reading
// ---------------------------------------------------------------------------

function expandEnvVars(value: string): string {
	let result = "";
	for (let i = 0; i < value.length; i++) {
		if (value[i] !== "$" || i === value.length - 1) {
			result += value[i];
			continue;
		}
		if (value[i + 1] === "{") {
			const end = value.indexOf("}", i + 2);
			if (end > i + 2) {
				const name = value.slice(i + 2, end);
				result += process.env[name] ?? value.slice(i, end + 1);
				i = end;
				continue;
			}
		}
		const start = i + 1;
		let end = start;
		while (end < value.length) {
			const code = value.charCodeAt(end);
			const isLetter = (code >= 65 && code <= 90) || (code >= 97 && code <= 122);
			const isDigit = code >= 48 && code <= 57;
			if (!isLetter && !isDigit && code !== 95) break;
			end++;
		}
		const name = value.slice(start, end);
		if (!name || !Number.isNaN(Number(name[0]))) {
			result += "$";
			continue;
		}
		result += process.env[name] ?? value.slice(i, end);
		i = end - 1;
	}
	return result;
}

/** Read a token from a file path (supports `~` and `$ENV_VAR` expansion). */
export function readAuthFile(filePath: string | null): string | null {
	if (!filePath) return null;
	// Expand ~ and $ENV_VAR
	let resolved = filePath.startsWith("~") ? `${codememHomeDir()}${filePath.slice(1)}` : filePath;
	resolved = expandEnvVars(resolved);
	try {
		if (!existsSync(resolved) || !statSync(resolved).isFile()) return null;
		const token = readFileSync(resolved, "utf-8").trim();
		return token || null;
	} catch {
		return null;
	}
}

// ---------------------------------------------------------------------------
// Source normalization
// ---------------------------------------------------------------------------

const VALID_SOURCES = new Set(["", "auto", "env", "file", "none"]);

function normalizeAuthSource(value: string | null | undefined): string {
	const normalized = (value ?? "").trim().toLowerCase();
	return VALID_SOURCES.has(normalized) ? normalized || "auto" : "none";
}

// ---------------------------------------------------------------------------
// Auth adapter (credential cascade with caching)
// ---------------------------------------------------------------------------

export interface ObserverAuthResolveOptions {
	explicitToken?: string | null;
	envTokens?: string[];
	forceRefresh?: boolean;
}

/**
 * Resolves auth credentials through a configurable cascade:
 * explicit → env → file.
 *
 * Results from the file source are cached for `cacheTtlS` seconds.
 */
export class ObserverAuthAdapter {
	readonly source: string;
	readonly filePath: string | null;
	readonly cacheTtlS: number;

	private cached: ObserverAuthMaterial = noAuth();
	private cachedAtMs = 0;

	constructor(opts?: {
		source?: string;
		filePath?: string | null;
		cacheTtlS?: number;
	}) {
		this.source = opts?.source ?? "auto";
		this.filePath = opts?.filePath ?? null;
		this.cacheTtlS = opts?.cacheTtlS ?? 300;
	}

	/** Resolve auth material through the credential cascade. */
	resolve(opts?: ObserverAuthResolveOptions): ObserverAuthMaterial {
		const source = normalizeAuthSource(this.source);
		const explicitToken = opts?.explicitToken ?? null;
		const envTokens = opts?.envTokens ?? [];
		const forceRefresh = opts?.forceRefresh ?? false;

		if (source === "none") return noAuth();

		if (!forceRefresh && source === "file" && this.cacheTtlS > 0) {
			const ageMs = performance.now() - this.cachedAtMs;
			if (this.cachedAtMs > 0 && ageMs <= this.cacheTtlS * 1000) {
				return this.cached;
			}
		}

		let token: string | null = null;
		let tokenSource = "none";

		if (source === "auto") {
			if (explicitToken) {
				token = explicitToken;
				tokenSource = "explicit";
			}
			if (!token) {
				token = envTokens.find((t) => !!t) ?? null;
				if (token) tokenSource = "env";
			}
		} else if (source === "env") {
			token = envTokens.find((t) => !!t) ?? null;
			if (token) tokenSource = "env";
		}

		if ((source === "auto" || source === "file") && !token) {
			token = readAuthFile(this.filePath);
			if (token) tokenSource = "file";
		}

		const resolved: ObserverAuthMaterial = token
			? { token, authType: "bearer", source: tokenSource }
			: noAuth();

		const shouldCache = source === "file";
		if (shouldCache && resolved.token) {
			this.cached = resolved;
			this.cachedAtMs = performance.now();
		} else if (shouldCache) {
			this.invalidateCache();
		}

		return resolved;
	}

	/** Clear the cached auth material. */
	invalidateCache(): void {
		this.cached = noAuth();
		this.cachedAtMs = 0;
	}
}

// ---------------------------------------------------------------------------
// Header rendering
// ---------------------------------------------------------------------------

const AUTH_TOKEN_RE = /\$\{auth\.token\}/g;
const AUTH_TYPE_RE = /\$\{auth\.type\}/g;
const AUTH_SOURCE_RE = /\$\{auth\.source\}/g;

/** Render observer headers with `${auth.token}`, `${auth.type}`, `${auth.source}` substitution. */
export function renderObserverHeaders(
	headers: Record<string, string>,
	auth: ObserverAuthMaterial,
): Record<string, string> {
	const token = auth.token ?? "";
	const rendered: Record<string, string> = {};
	for (const [key, value] of Object.entries(headers)) {
		if (typeof key !== "string" || typeof value !== "string") continue;

		let candidate = value.replaceAll(AUTH_TOKEN_RE, token);
		candidate = candidate.replaceAll(AUTH_TYPE_RE, auth.authType);
		candidate = candidate.replaceAll(AUTH_SOURCE_RE, auth.source);

		// Skip headers that reference auth.token when no token is available
		// biome-ignore lint/suspicious/noTemplateCurlyInString: intentional template pattern, not JS template literal
		if (value.includes("${auth.token}") && !token) continue;

		const cleaned = candidate.trim();
		if (!cleaned) continue;
		rendered[key] = cleaned;
	}
	return rendered;
}
