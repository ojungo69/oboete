/** Read one Codex hook payload and deliver it through the local daemon. */

import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import { addDbOption, addViewerHostOptions, type DbOpts, resolveDbOpt } from "../shared-options.js";
import { deliverHookEvent, prepareHookEvent } from "./hook-rpc-client.js";

type IngestVia = "rpc" | "spool" | "skipped" | "dropped";
type IngestResult = { inserted: number; skipped: number; via: IngestVia };
type IngestOpts = { host: string; port: string | number; deadlineAtMs?: number } & DbOpts;
type IngestDeps = { deliver?: typeof deliverHookEvent };

function emitHookContinue(): void {
	console.log(JSON.stringify({ continue: true }));
}

function logHookDiagnostic(message: string): void {
	console.error(`[codemem] codex-hook-ingest: ${message}`);
}

function envTruthyValue(value: string | undefined): boolean {
	const normalized = String(value ?? "")
		.trim()
		.toLowerCase();
	return normalized === "1" || normalized === "true" || normalized === "yes" || normalized === "on";
}

function normalizePayloadForIngest(payload: Record<string, unknown>): Record<string, unknown> {
	if (
		(typeof payload.timestamp === "string" && payload.timestamp.trim()) ||
		(typeof payload.ts === "string" && payload.ts.trim())
	) {
		return payload;
	}
	return {
		...payload,
		timestamp: new Date().toISOString(),
		codemem_generated_event_nonce: randomUUID(),
	};
}

export async function ingestCodexHookPayload(
	payload: Record<string, unknown>,
	_opts: IngestOpts,
	deps: IngestDeps = {},
): Promise<IngestResult> {
	const normalized = normalizePayloadForIngest(payload);
	const prepared = prepareHookEvent("codex", normalized, _opts.deadlineAtMs);
	const result = await (deps.deliver ?? deliverHookEvent)("codex", normalized, {
		prepared,
		dbPath: resolveDbOpt(_opts),
	});
	return {
		inserted: result.via === "rpc" ? 1 : 0,
		skipped: result.via === "skipped" ? 1 : 0,
		via: result.via,
	};
}

const codexHookCmd = new Command("codex-hook-ingest")
	.configureHelp(helpStyle)
	.description("Ingest a Codex hook payload through the local codemem daemon");

// Keep legacy flags accepted while hook clients stop opening SQLite or HTTP directly.
addDbOption(codexHookCmd);
addViewerHostOptions(codexHookCmd);

export const codexHookIngestCommand = codexHookCmd.action(
	async (opts: DbOpts & { host: string; port: string }) => {
		if (envTruthyValue(process.env.CODEMEM_PLUGIN_IGNORE)) {
			emitHookContinue();
			return;
		}

		let raw: string;
		try {
			raw = readFileSync(0, "utf8").trim();
		} catch {
			logHookDiagnostic("failed to read stdin");
			emitHookContinue();
			return;
		}
		if (!raw) {
			emitHookContinue();
			return;
		}

		let payload: Record<string, unknown>;
		try {
			const parsed = JSON.parse(raw) as unknown;
			if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) {
				logHookDiagnostic("payload must be a JSON object");
				emitHookContinue();
				return;
			}
			payload = parsed as Record<string, unknown>;
		} catch {
			logHookDiagnostic("invalid JSON payload");
			emitHookContinue();
			return;
		}

		try {
			logHookDiagnostic(JSON.stringify(await ingestCodexHookPayload(payload, opts)));
		} catch {
			logHookDiagnostic("ingest failed");
		}
		emitHookContinue();
	},
);
