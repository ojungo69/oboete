import { homedir } from "node:os";
import { fingerprintSecretRules } from "./gitleaks-pinned-rules.js";
import { isSensitiveFieldName } from "./ingest-sanitize.js";
import {
	applyPrivateRegexInWorker,
	prepareRedactionWorkerForScan,
	redactValueInWorker,
} from "./redaction-worker.js";
import { DEFAULT_RULES, type ScanDetection, type SecretRule } from "./secret-scanner.js";

export type Sensitivity = "normal" | "private" | "secret";

export type AgentMemoryConfig = {
	ignorePaths: string[];
	localOnlyPaths: string[];
	privateRegex: string[];
	secretRules: SecretRule[];
	toolFieldAllowlist: string[];
	toolFieldDenylist: string[];
	remoteProcessing: boolean;
	warnings: string[];
	degraded: boolean;
};

export type RedactionOptions = {
	config?: AgentMemoryConfig;
	allowlist?: string[];
	metadataKeys?: string[];
	maxBytes?: number;
	workerStartupDeadlineAtMs?: number;
};

export type RedactionResult = {
	payload: Record<string, unknown>;
	sensitivity: Sensitivity;
	secret_rules_version: string;
	degraded: boolean;
	private_content_omitted: boolean;
	local_only: boolean;
	detections: ScanDetection[];
	warnings: string[];
};

const DEFAULT_ALLOWLIST = [
	"id",
	"type",
	"body",
	"title",
	"text",
	"path",
	"tool",
	"prompt",
	"output",
	"input",
	"note",
	"narrative",
	"content",
];
const PATH_KEYS = new Set(["path", "file_path", "cwd", "cwd_path"]);
const SECRET_BODY_KEYS = new Set([
	"body",
	"title",
	"text",
	"prompt",
	"output",
	"input",
	"note",
	"narrative",
	"content",
]);
const EVENT_MAX_BYTES = 32 * 1024;
const FIELD_MAX_BYTES = 16 * 1024;
const USER_RULE_MAX = 100;
const USER_PATTERN_MAX = 512;

const KNOWN_TOML_KEYS = new Set([
	"ignore_paths",
	"local_only_paths",
	"private_regex",
	"secret_regex",
	"tool_field_allowlist",
	"tool_field_denylist",
	"remote_processing",
]);

export function parseAgentMemoryToml(source: string): AgentMemoryConfig {
	const warnings: string[] = [];
	const parsed = new Map<string, unknown>();
	for (const rawLine of source.split(/\r?\n/)) {
		const line = rawLine.trim();
		if (!line || line.startsWith("#")) continue;
		const eq = line.indexOf("=");
		if (eq < 0) {
			warnings.push("ignored malformed line");
			continue;
		}
		const key = line.slice(0, eq).trim();
		const value = parseTomlValue(line.slice(eq + 1).trim());
		if (!KNOWN_TOML_KEYS.has(key)) {
			warnings.push("unknown key");
			continue;
		}
		parsed.set(key, value);
	}

	let degraded = false;
	const strings = (key: string): string[] => {
		const value = parsed.get(key);
		if (value === undefined) return [];
		if (Array.isArray(value) && value.every((item) => typeof item === "string")) {
			return value;
		}
		warnings.push(`${key} must be an array of strings`);
		degraded = true;
		return [];
	};

	let remoteProcessing = false;
	if (parsed.has("remote_processing")) {
		const value = parsed.get("remote_processing");
		if (typeof value === "boolean") remoteProcessing = value;
		else {
			warnings.push("remote_processing must be a boolean");
			remoteProcessing = false;
			degraded = true;
		}
	}

	const compiled = compileUserRules(strings("secret_regex"), warnings);
	const privatePatterns = compilePrivatePatterns(strings("private_regex"), warnings);
	degraded = degraded || compiled.degraded || privatePatterns.degraded;
	const rules = compiled.rules;
	// ignorePaths / localOnlyPaths are parsed fail-closed here; path policy apply is T041.
	return {
		ignorePaths: strings("ignore_paths"),
		localOnlyPaths: strings("local_only_paths"),
		privateRegex: privatePatterns.patterns,
		secretRules: rules,
		toolFieldAllowlist: strings("tool_field_allowlist"),
		toolFieldDenylist: strings("tool_field_denylist"),
		remoteProcessing,
		warnings,
		degraded,
	};
}

export function preprocessAdapterEvent(
	input: unknown,
	options: RedactionOptions = {},
): RedactionResult {
	return runPipeline(input, options, "adapter");
}

export function applyDaemonIntake(input: unknown, options: RedactionOptions = {}): RedactionResult {
	return runPipeline(input, options, "intake");
}

function runPipeline(
	input: unknown,
	options: RedactionOptions,
	layer: "adapter" | "intake",
): RedactionResult {
	const config = options.config;
	const allow = resolveAllowlist(options.allowlist);
	const toolAllow = new Set(config?.toolFieldAllowlist ?? []);
	const toolDeny = new Set(config?.toolFieldDenylist ?? []);
	const source =
		input && typeof input === "object" && !Array.isArray(input)
			? (input as Record<string, unknown>)
			: {};
	let payload: Record<string, unknown> = {};
	if (tooLarge(source)) {
		payload = {};
	} else {
		for (const [key, value] of Object.entries(source)) {
			if (allow.has(key) && !isSensitiveFieldName(key)) {
				payload[key] = dropSensitiveFields(value, toolAllow, toolDeny);
			}
		}
	}

	payload = mapStrings(payload, stripInjectedContext);
	payload = mapStrings(payload, (text, key) =>
		PATH_KEYS.has(key) ? normalizePathValue(text) : text,
	);

	const userRules = config?.secretRules ?? [];
	const rules = [...DEFAULT_RULES, ...userRules];
	const workerDeadlineAtMs = prepareRedactionWorkerForScan(options.workerStartupDeadlineAtMs);
	const scanDeadlineAtMs = workerDeadlineAtMs ?? 0;
	const firstScan =
		workerDeadlineAtMs !== null
			? redactValueInWorker(payload, userRules, scanDeadlineAtMs)
			: { ok: false as const };
	const loadedRules = firstScan.ok ? rules : [];
	let workerDegraded = !firstScan.ok;
	let detections = firstScan.ok ? firstScan.detections : [];
	payload = firstScan.ok
		? asObject(firstScan.value)
		: keepMetadataOnly(payload, options.metadataKeys);

	let privateOmitted = false;
	let localOnly = false;
	payload = mapStrings(payload, (text) => {
		const stripped = stripReservedMarkup(text);
		if (stripped.privateHit) privateOmitted = true;
		if (stripped.localOnly) localOnly = true;
		return stripped.text;
	});
	if (config?.privateRegex.length && !workerDegraded) {
		const metadata = keepMetadataOnly(payload, options.metadataKeys);
		const privateScan = applyPrivateRegexInWorker(payload, config.privateRegex, scanDeadlineAtMs);
		if (privateScan.ok) {
			payload = asObject(privateScan.value);
			privateOmitted ||= privateScan.privateHit;
		} else {
			payload = metadata;
			privateOmitted = true;
			workerDegraded = true;
		}
	}
	payload = mapStrings(payload, (text) => {
		const again = stripReservedMarkup(text);
		if (again.privateHit) privateOmitted = true;
		if (again.localOnly) localOnly = true;
		return again.text;
	});

	// Tags can reassemble split secrets; scan again after markup removal.
	if (!workerDegraded) {
		const secondScan = redactValueInWorker(payload, userRules, scanDeadlineAtMs);
		if (secondScan.ok) {
			payload = asObject(secondScan.value);
			detections = mergeDetections(detections, secondScan.detections);
		} else {
			payload = keepMetadataOnly(payload, options.metadataKeys);
			workerDegraded = true;
		}
	}
	payload = boundSize(payload, options.maxBytes ?? EVENT_MAX_BYTES);
	if (detections.length > 0) {
		process.stderr.write(
			`[redaction] layer=${layer} kinds=${detections.map((item) => item.kind).join(",")}\n`,
		);
	}

	let sensitivity: Sensitivity = "normal";
	if (detections.length > 0) sensitivity = "secret";
	else if (privateOmitted) sensitivity = "private";

	if (layer === "intake" && detections.length > 0) {
		payload = keepMetadataOnly(payload, options.metadataKeys);
		sensitivity = "secret";
	}
	const degraded = Boolean(config?.degraded) || workerDegraded;
	if (degraded) {
		payload = keepMetadataOnly(payload, options.metadataKeys);
	}
	payload = enforceMaxBytes(payload, options.maxBytes ?? EVENT_MAX_BYTES, options.metadataKeys);

	return {
		payload,
		sensitivity,
		secret_rules_version: fingerprintSecretRules(loadedRules, degraded),
		degraded,
		private_content_omitted: privateOmitted,
		local_only: localOnly,
		detections,
		warnings: workerDegraded
			? [...(config?.warnings ?? []), "redaction worker deadline exceeded"]
			: (config?.warnings ?? []),
	};
}

function compileUserRules(
	patterns: string[],
	warnings: string[],
): { rules: SecretRule[]; degraded: boolean } {
	const rules: SecretRule[] = [];
	let degraded = false;
	for (const pattern of patterns.slice(0, USER_RULE_MAX)) {
		if (pattern.length > USER_PATTERN_MAX) {
			warnings.push("secret_regex pattern exceeds 512 characters");
			degraded = true;
			continue;
		}
		try {
			rules.push({
				kind: `user_${rules.length + 1}`,
				pattern: new RegExp(pattern, "g"),
				origin: "user",
			});
		} catch {
			warnings.push("secret_regex pattern is invalid");
			degraded = true;
		}
	}
	if (patterns.length > USER_RULE_MAX) {
		warnings.push("secret_regex exceeds 100 patterns");
		degraded = true;
	}
	return { rules, degraded };
}

function compilePrivatePatterns(
	patterns: string[],
	warnings: string[],
): { patterns: string[]; degraded: boolean } {
	const valid: string[] = [];
	let degraded = false;
	for (const pattern of patterns.slice(0, USER_RULE_MAX)) {
		if (pattern.length > USER_PATTERN_MAX) {
			warnings.push("private_regex pattern exceeds 512 characters");
			degraded = true;
			continue;
		}
		try {
			// Validation is length-bounded here; matching runs in the deadline-bounded worker.
			new RegExp(pattern, "g"); // nosemgrep
			valid.push(pattern);
		} catch {
			warnings.push("private_regex pattern is invalid");
			degraded = true;
		}
	}
	if (patterns.length > USER_RULE_MAX) {
		warnings.push("private_regex exceeds 100 patterns");
		degraded = true;
	}
	return { patterns: valid, degraded };
}

function parseTomlValue(raw: string): unknown {
	if (raw === "true") return true;
	if (raw === "false") return false;
	if (/^-?\d+(?:\.\d+)?$/.test(raw)) return Number(raw);
	if (raw.startsWith("[")) {
		try {
			const parsed = JSON.parse(raw.replaceAll("'", '"'));
			if (Array.isArray(parsed)) return parsed;
		} catch {
			return Symbol.for("invalid");
		}
	}
	if ((raw.startsWith('"') && raw.endsWith('"')) || (raw.startsWith("'") && raw.endsWith("'"))) {
		return raw.slice(1, -1);
	}
	return raw;
}

function resolveAllowlist(requested: string[] | undefined): Set<string> {
	return new Set(requested ?? DEFAULT_ALLOWLIST);
}

function dropSensitiveFields(
	value: unknown,
	toolAllow: Set<string>,
	toolDeny: Set<string>,
	insideToolInput = false,
	restrictToAllowlist = false,
): unknown {
	if (Array.isArray(value)) {
		return value.map((item) => dropSensitiveFields(item, toolAllow, toolDeny, insideToolInput));
	}
	if (value && typeof value === "object") {
		const next: Record<string, unknown> = {};
		for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
			if (
				toolDeny.has(key) ||
				(restrictToAllowlist && toolAllow.size > 0 && !toolAllow.has(key)) ||
				isSensitiveFieldName(key) ||
				/<\/?(?:private|local-only|injected-context)>/i.test(key)
			) {
				continue;
			}
			const startsToolInput = key === "tool_input";
			next[key] = dropSensitiveFields(
				child,
				toolAllow,
				toolDeny,
				insideToolInput || startsToolInput,
				startsToolInput,
			);
		}
		return next;
	}
	return value;
}

function mergeDetections(...groups: ScanDetection[][]): ScanDetection[] {
	const counts = new Map<string, number>();
	for (const group of groups) {
		for (const item of group) {
			counts.set(item.kind, (counts.get(item.kind) ?? 0) + item.count);
		}
	}
	return Array.from(counts.entries()).map(([kind, count]) => ({ kind, count }));
}

function keepMetadataOnly(
	payload: Record<string, unknown>,
	metadataKeys: string[] = ["id", "type"],
): Record<string, unknown> {
	const next: Record<string, unknown> = {};
	for (const key of metadataKeys) {
		if (Object.hasOwn(payload, key)) next[key] = payload[key];
	}
	return next;
}

function enforceMaxBytes(
	payload: Record<string, unknown>,
	maxBytes: number,
	metadataKeys?: string[],
): Record<string, unknown> {
	try {
		if (Buffer.byteLength(JSON.stringify(payload), "utf8") <= maxBytes) return payload;
	} catch {
		return keepMetadataOnly(payload, metadataKeys);
	}
	const trimmed = { ...payload };
	for (const key of SECRET_BODY_KEYS) delete trimmed[key];
	try {
		if (Buffer.byteLength(JSON.stringify(trimmed), "utf8") <= maxBytes) return trimmed;
	} catch {
		return {};
	}
	const meta = keepMetadataOnly(trimmed, metadataKeys);
	try {
		if (Buffer.byteLength(JSON.stringify(meta), "utf8") <= maxBytes) return meta;
	} catch {
		return {};
	}
	return {};
}

function boundSize(payload: Record<string, unknown>, maxBytes: number): Record<string, unknown> {
	const next = mapStrings(payload, (text) =>
		text.length > FIELD_MAX_BYTES ? elide(text, text.length) : text,
	);
	const encoded = Buffer.byteLength(JSON.stringify(next), "utf8");
	if (encoded <= maxBytes) return next;
	return mapStrings(next, (text) => elide(text, text.length));
}

function elide(text: string, original: number): string {
	const head = 256;
	const tail = 256;
	if (text.length <= head + tail) return text;
	return `${text.slice(0, head)}\n…[elided ${original} bytes]…\n${text.slice(-tail)}`;
}

function stripInjectedContext(text: string): string {
	return stripTagged(text, "injected-context", "drop").text;
}

function stripReservedMarkup(text: string): {
	text: string;
	privateHit: boolean;
	localOnly: boolean;
} {
	let current = text;
	let privateHit = false;
	let localOnly = false;
	for (let i = 0; i < 8; i += 1) {
		const injected = stripTagged(current, "injected-context", "drop");
		const priv = stripTagged(injected.text, "private", "drop");
		if (priv.hit) privateHit = true;
		const local = stripTagged(priv.text, "local-only", "keep");
		if (local.hit) localOnly = true;
		if (local.text === current) break;
		if (local.text.length > current.length) {
			return { text: "", privateHit: true, localOnly };
		}
		current = local.text;
	}
	if (/<\/?(?:private|injected-context)>/i.test(current)) {
		return { text: "", privateHit: true, localOnly };
	}
	return { text: current, privateHit, localOnly };
}

function tooLarge(payload: Record<string, unknown>): boolean {
	let nodes = 0;
	const walk = (value: unknown, depth: number): boolean => {
		if (depth > 32) return true;
		nodes += 1;
		if (nodes > 2048) return true;
		if (Array.isArray(value)) return value.some((item) => walk(item, depth + 1));
		if (value && typeof value === "object") {
			return Object.values(value as Record<string, unknown>).some((item) => walk(item, depth + 1));
		}
		return false;
	};
	return walk(payload, 0);
}

function normalizePathValue(value: string): string {
	if (value === "~") return homedir();
	if (value.startsWith("~/")) return `${homedir()}/${value.slice(2)}`;
	return value;
}

/**
 * Matchers for the three reserved tags, compiled once.
 *
 * `nextTag` runs inside `stripTagged`'s scan loop, so building these from the tag name
 * compiled two patterns per iteration - on the path every captured string goes through.
 * The tag set is closed (the only callers pass these three literals), so a fixed table is
 * both cheaper and narrower: `ReservedTag` now rejects anything else at the type level,
 * and no `RegExp` is constructed from a value.
 *
 * `either` carries `g` because it is used with `String.prototype.replace`, which resets
 * `lastIndex` around the call, so sharing one instance across calls carries no state.
 * `open` and `close` have no `g`, so `exec` ignores `lastIndex` entirely.
 */
const RESERVED_TAG_PATTERNS = {
	private: { open: /<private>/i, close: /<\/private>/i, either: /<\/?private>/gi },
	"local-only": { open: /<local-only>/i, close: /<\/local-only>/i, either: /<\/?local-only>/gi },
	"injected-context": {
		open: /<injected-context>/i,
		close: /<\/injected-context>/i,
		either: /<\/?injected-context>/gi,
	},
} as const;

type ReservedTag = keyof typeof RESERVED_TAG_PATTERNS;

function nextTag(
	text: string,
	tag: ReservedTag,
	from: number,
): { kind: "open" | "close"; index: number; length: number } | null {
	const slice = text.slice(from);
	const patterns = RESERVED_TAG_PATTERNS[tag];
	const openMatch = patterns.open.exec(slice);
	const closeMatch = patterns.close.exec(slice);
	const openAt = openMatch?.index ?? -1;
	const closeAt = closeMatch?.index ?? -1;
	if (openAt < 0 && closeAt < 0) return null;
	if (closeAt >= 0 && (openAt < 0 || closeAt < openAt)) {
		return { kind: "close", index: from + closeAt, length: closeMatch?.[0].length ?? 0 };
	}
	return { kind: "open", index: from + openAt, length: openMatch?.[0].length ?? 0 };
}

function stripTagged(
	text: string,
	tag: ReservedTag,
	unclosed: "drop" | "keep",
): { text: string; hit: boolean } {
	let cursor = 0;
	let output = "";
	let hit = false;
	while (cursor < text.length) {
		const found = nextTag(text, tag, cursor);
		if (!found) {
			output += text.slice(cursor);
			break;
		}
		if (found.kind === "close") {
			// Orphan close tag: whatever region it belongs to is unknown, since the opener may
			// have been truncated away or removed by an earlier reserved-tag pass. Fail closed
			// exactly as before, but leave `[/tag]` so the omission is visible instead of silent
			// (#117). The marker is the same length as the `</tag>` it replaces and contains no
			// `<`, so it trips neither guard in stripReservedMarkup.
			hit = true;
			output += unclosed === "keep" ? text.slice(cursor, found.index) : `[/${tag}]`;
			cursor = found.index + found.length;
			continue;
		}
		hit = true;
		output += text.slice(cursor, found.index);
		let pos = found.index + found.length;
		let depth = 1;
		const innerStart = pos;
		while (depth > 0) {
			const inner = nextTag(text, tag, pos);
			if (!inner) {
				// Unclosed open tag: the block's extent runs to the end of the string. `[tag]`
				// marks the omission, and is the same length as the `<tag>` it replaces. The
				// `keep` side strips the tag's own markup out of what it keeps, matching the
				// closed-block branch below - nested opens were consumed by `depth` above and
				// would otherwise survive into the output.
				return {
					text:
						unclosed === "keep"
							? output + text.slice(innerStart).replace(RESERVED_TAG_PATTERNS[tag].either, "")
							: `${output}[${tag}]`,
					hit,
				};
			}
			if (inner.kind === "open") {
				depth += 1;
				pos = inner.index + inner.length;
				continue;
			}
			depth -= 1;
			pos = inner.index + inner.length;
		}
		if (unclosed === "keep") {
			output += text.slice(innerStart, pos).replace(RESERVED_TAG_PATTERNS[tag].either, "");
		}
		cursor = pos;
	}
	return { text: output, hit };
}

function mapStrings(
	value: Record<string, unknown>,
	fn: (text: string, key: string) => string,
): Record<string, unknown> {
	const walk = (item: unknown, key: string): unknown => {
		if (typeof item === "string") return fn(item, key);
		if (Array.isArray(item)) return item.map((entry) => walk(entry, key));
		if (item && typeof item === "object") {
			const next: Record<string, unknown> = {};
			for (const [childKey, child] of Object.entries(item as Record<string, unknown>)) {
				next[childKey] = walk(child, childKey);
			}
			return next;
		}
		return item;
	};
	return walk(value, "") as Record<string, unknown>;
}

function asObject(value: unknown): Record<string, unknown> {
	if (value && typeof value === "object" && !Array.isArray(value)) {
		return value as Record<string, unknown>;
	}
	return {};
}
