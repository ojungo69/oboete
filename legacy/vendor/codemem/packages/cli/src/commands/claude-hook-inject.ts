import { resolveHookProject } from "@codemem/core";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import { addDbOption, type DbOpts, resolveDbOpt } from "../shared-options.js";
import { logHookEvent } from "./claude-hook-plugin-log.js";
import {
	buildInjectQuery,
	normalizePromptText,
	type SessionState,
	trackHookSessionState,
	workingSetPathsFromState,
} from "./claude-hook-session-state.js";
import {
	deliverHookEvent,
	type HookPackResult,
	prepareHookEvent,
	requestHookPack,
} from "./hook-rpc-client.js";

type InjectResult = {
	continue: true;
	hookSpecificOutput?: {
		hookEventName: "UserPromptSubmit";
		additionalContext: string;
	};
};

const HOOK_EVENT_NAME = "UserPromptSubmit" as const;
type InjectOpts = DbOpts & { deadlineAtMs?: number };
export type PackResult = HookPackResult;
type InjectDeps = {
	requestPack?: typeof requestHookPack;
	deliver?: typeof deliverHookEvent;
};

const EMPTY_PACK: PackResult = { packText: "", items: 0, packTokens: 0 };
const DEFAULT_MAX_CHARS = 16_000;

function emitJson(value: InjectResult): void {
	console.log(JSON.stringify(value));
}

function envNotDisabled(value: string | undefined): boolean {
	const normalized = String(value ?? "")
		.trim()
		.toLowerCase();
	return normalized !== "0" && normalized !== "false" && normalized !== "off";
}

function envTruthy(value: string | undefined): boolean {
	const normalized = String(value ?? "")
		.trim()
		.toLowerCase();
	return normalized === "1" || normalized === "true" || normalized === "yes" || normalized === "on";
}

function parsePositiveInt(value: string | undefined, fallback: number): number {
	const parsed = Number.parseInt(String(value ?? ""), 10);
	return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function continueResult(additionalContext?: string): InjectResult {
	return additionalContext
		? {
				continue: true,
				hookSpecificOutput: { hookEventName: HOOK_EVENT_NAME, additionalContext },
			}
		: { continue: true };
}

function truncateAdditionalContext(text: string, maxChars: number): string {
	const normalized = text.trim();
	if (!normalized || normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, maxChars).trimEnd()}\n\n[pack truncated]`;
}

function resolveInjectProject(payload: Record<string, unknown>): string | null {
	const cwd = typeof payload.cwd === "string" ? payload.cwd : null;
	return resolveHookProject(cwd, payload.project);
}

export async function buildClaudeHookInjection(
	payload: Record<string, unknown>,
	_opts: InjectOpts,
	deps: InjectDeps = {},
): Promise<InjectResult> {
	if (envTruthy(process.env.CODEMEM_PLUGIN_IGNORE)) return continueResult();
	const prepared = prepareHookEvent("claude", payload, _opts.deadlineAtMs);
	if (prepared.status === "skipped") return continueResult();
	const deliveryOptions = { prepared, dbPath: resolveDbOpt(_opts) };

	let state: SessionState | null = null;
	try {
		state = trackHookSessionState(
			{
				session_id: prepared.event.nativeSessionId,
				hook_event_name: payload.hook_event_name,
			},
			prepared.redaction.local_only ? "" : prepared.safePrompt,
			[],
		);
	} catch {
		// Query enrichment is best effort.
	}
	const prompt = normalizePromptText(prepared.safePrompt);
	const deliver = deps.deliver ?? deliverHookEvent;
	if (!prompt) {
		await deliver("claude", payload, deliveryOptions).catch(() => ({ via: "dropped" as const }));
		return continueResult();
	}
	if (!envNotDisabled(process.env.CODEMEM_INJECT_CONTEXT || "1")) {
		await deliver("claude", payload, deliveryOptions).catch(() => ({ via: "dropped" as const }));
		return continueResult();
	}
	if (prepared.redaction.local_only) {
		await deliver("claude", payload, deliveryOptions).catch(() => ({ via: "dropped" as const }));
		return continueResult();
	}

	const project = resolveInjectProject(payload);
	const query = buildInjectQuery({ prompt, project, state });
	const requestPack = deps.requestPack ?? requestHookPack;
	const [pack] = await Promise.all([
		requestPack(
			"claude",
			{
				context: query,
				project,
				workingSetPaths: workingSetPathsFromState(state),
				limit: parsePositiveInt(process.env.CODEMEM_INJECT_LIMIT, 8),
				tokenBudget: parsePositiveInt(process.env.CODEMEM_INJECT_TOKEN_BUDGET, 800),
			},
			{
				config: prepared.config,
				deadlineAtMs: prepared.deadlineAtMs,
				dbPath: deliveryOptions.dbPath,
			},
		).catch(() => EMPTY_PACK),
		deliver("claude", payload, deliveryOptions).catch(() => ({ via: "dropped" as const })),
	]);

	const fields = [
		"inject.pack.ok",
		"source=claude",
		`origin=${pack.packText ? "rpc" : "none"}`,
		`items=${pack.items}`,
		`pack_tokens=${pack.packTokens}`,
		`query_len=${query.length}`,
		`empty=${pack.packText ? "false" : "true"}`,
	];
	logHookEvent(fields.join(" "));
	return continueResult(
		truncateAdditionalContext(
			pack.packText,
			parsePositiveInt(process.env.CODEMEM_INJECT_MAX_CHARS, DEFAULT_MAX_CHARS),
		),
	);
}

const claudeHookInjectCmd = new Command("claude-hook-inject")
	.configureHelp(helpStyle)
	.description("Return Claude hook additionalContext from the local codemem daemon");

// Keep the legacy flag accepted; the hook no longer opens SQLite itself.
addDbOption(claudeHookInjectCmd);

export const claudeHookInjectCommand = claudeHookInjectCmd.action(async (opts: InjectOpts) => {
	let raw = "";
	for await (const chunk of process.stdin) raw += String(chunk);
	try {
		const parsed = JSON.parse(raw.trim()) as unknown;
		if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error();
		emitJson(await buildClaudeHookInjection(parsed as Record<string, unknown>, opts));
	} catch {
		emitJson(continueResult());
	}
});
