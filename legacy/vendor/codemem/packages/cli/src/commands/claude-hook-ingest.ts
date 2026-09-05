/** Read one Claude Code hook payload and deliver it through the local daemon. */

import { readFileSync } from "node:fs";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import { addDbOption, addViewerHostOptions, type DbOpts, resolveDbOpt } from "../shared-options.js";
import {
	extractModifiedPathsFromHook,
	trackHookSessionState,
} from "./claude-hook-session-state.js";
import { deliverHookEvent, prepareHookEvent } from "./hook-rpc-client.js";

type IngestVia = "rpc" | "spool" | "skipped" | "dropped";
type IngestResult = { inserted: number; skipped: number; via: IngestVia };
type IngestOpts = { host: string; port: string | number; deadlineAtMs?: number } & DbOpts;
type IngestDeps = { deliver?: typeof deliverHookEvent };

function emitStructuredError(errorCode: string, message: string): void {
	console.log(JSON.stringify({ error: errorCode, message }));
}

export async function ingestClaudeHookPayload(
	payload: Record<string, unknown>,
	_opts: IngestOpts,
	deps: IngestDeps = {},
): Promise<IngestResult> {
	const prepared = prepareHookEvent("claude", payload, _opts.deadlineAtMs);
	if (prepared.status === "ready") {
		try {
			const eventPayload = prepared.event.payload;
			const adapter =
				eventPayload && typeof eventPayload === "object" && !Array.isArray(eventPayload)
					? (eventPayload as Record<string, unknown>)._adapter
					: null;
			const adapterPayload =
				adapter && typeof adapter === "object" && !Array.isArray(adapter)
					? (adapter as Record<string, unknown>).payload
					: null;
			const rawPaths = extractModifiedPathsFromHook(payload);
			const redactedPaths =
				adapterPayload && typeof adapterPayload === "object" && !Array.isArray(adapterPayload)
					? extractModifiedPathsFromHook(adapterPayload as Record<string, unknown>)
					: [];
			const safePaths = redactedPaths.filter((path) => rawPaths.includes(path));
			trackHookSessionState(
				{
					session_id: prepared.event.nativeSessionId,
					hook_event_name: payload.hook_event_name,
				},
				prepared.redaction.local_only ? "" : prepared.safePrompt,
				prepared.redaction.local_only ? [] : safePaths,
			);
		} catch {
			// Session-state enrichment is best effort and must not block capture.
		}
	}
	const result = await (deps.deliver ?? deliverHookEvent)("claude", payload, {
		prepared,
		dbPath: resolveDbOpt(_opts),
	});
	return {
		inserted: result.via === "rpc" ? 1 : 0,
		skipped: result.via === "skipped" ? 1 : 0,
		via: result.via,
	};
}

const claudeHookCmd = new Command("claude-hook-ingest")
	.configureHelp(helpStyle)
	.description("Ingest a Claude hook payload through the local codemem daemon");

// Keep legacy flags accepted while hook clients stop opening SQLite or HTTP directly.
addDbOption(claudeHookCmd);
addViewerHostOptions(claudeHookCmd);

function envTruthyValue(value: string | undefined): boolean {
	const normalized = String(value ?? "")
		.trim()
		.toLowerCase();
	return normalized === "1" || normalized === "true" || normalized === "yes" || normalized === "on";
}

export const claudeHookIngestCommand = claudeHookCmd.action(
	async (opts: DbOpts & { host: string; port: string }) => {
		if (envTruthyValue(process.env.CODEMEM_PLUGIN_IGNORE)) return;

		let raw: string;
		try {
			raw = readFileSync(0, "utf8").trim();
		} catch {
			emitStructuredError("read_error", "failed to read stdin");
			return;
		}
		if (!raw) {
			emitStructuredError("read_error", "empty stdin");
			return;
		}

		let payload: Record<string, unknown>;
		try {
			const parsed = JSON.parse(raw) as unknown;
			if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) {
				emitStructuredError("parse_error", "payload must be a JSON object");
				return;
			}
			payload = parsed as Record<string, unknown>;
		} catch {
			emitStructuredError("parse_error", "invalid JSON");
			return;
		}

		try {
			console.log(JSON.stringify(await ingestClaudeHookPayload(payload, opts)));
		} catch {
			console.log(JSON.stringify({ inserted: 0, skipped: 0, via: "dropped" }));
		}
	},
);
