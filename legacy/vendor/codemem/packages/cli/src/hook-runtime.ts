#!/usr/bin/env node

import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";
import { HOOK_DELIVERY_BUDGETS } from "@codemem/core";
import { buildClaudeFileContext } from "./commands/claude-hook-file-context.js";
import { ingestClaudeHookPayload } from "./commands/claude-hook-ingest.js";
import { buildClaudeHookInjection } from "./commands/claude-hook-inject.js";
import { ingestCodexHookPayload } from "./commands/codex-hook-ingest.js";
import { buildCodexHookInjection } from "./commands/codex-hook-inject.js";

export const HOOK_RUNTIME_INPUT_MAX_BYTES = 256 * 1024;
const HOOK_RUNTIME_OUTPUT_MAX_BYTES = 256 * 1024;

const CONTINUE = '{"continue":true}';
const COMMANDS = new Set([
	"claude-hook-file-context",
	"claude-hook-ingest",
	"claude-hook-inject",
	"codex-hook-ingest",
	"codex-hook-inject",
]);

type HookRuntimeWorkerData = {
	role: "hook-runtime";
	command: string;
	raw: string;
	deadlineAtMs: number;
};

function fallback(command: string): string {
	return command === "claude-hook-ingest" ? "" : CONTINUE;
}

function disabled(): boolean {
	return ["1", "true", "yes", "on"].includes(
		String(process.env.CODEMEM_PLUGIN_IGNORE ?? "")
			.trim()
			.toLowerCase(),
	);
}

export async function runHookRuntime(
	command: string,
	raw: string,
	deadlineAtMs?: number,
): Promise<string> {
	if (!COMMANDS.has(command)) throw new Error("unsupported hook command");
	if (disabled() || Buffer.byteLength(raw, "utf8") > HOOK_RUNTIME_INPUT_MAX_BYTES) {
		return fallback(command);
	}
	let payload: Record<string, unknown>;
	try {
		const parsed = JSON.parse(raw.trim()) as unknown;
		if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return fallback(command);
		payload = parsed as Record<string, unknown>;
	} catch {
		return fallback(command);
	}
	try {
		if (command === "claude-hook-ingest") {
			await ingestClaudeHookPayload(payload, {
				host: "127.0.0.1",
				port: 38888,
				deadlineAtMs,
			});
			return "";
		}
		if (command === "codex-hook-ingest") {
			await ingestCodexHookPayload(payload, {
				host: "127.0.0.1",
				port: 38888,
				deadlineAtMs,
			});
			return CONTINUE;
		}
		if (command === "claude-hook-inject") {
			return JSON.stringify(await buildClaudeHookInjection(payload, { deadlineAtMs }));
		}
		if (command === "codex-hook-inject") {
			return JSON.stringify(await buildCodexHookInjection(payload, { deadlineAtMs }));
		}
		return JSON.stringify(await buildClaudeFileContext(payload, { deadlineAtMs }));
	} catch {
		return fallback(command);
	}
}

function clientHardCapMs(command: string): number {
	return command.startsWith("claude-")
		? HOOK_DELIVERY_BUDGETS.claude.clientHardCapMs
		: HOOK_DELIVERY_BUDGETS.codex.clientHardCapMs;
}

async function readStdin(deadlineAtMs: number): Promise<string | null> {
	const chunks: Buffer[] = [];
	let size = 0;
	const timeout = setTimeout(
		() => process.stdin.destroy(new Error("hook stdin deadline exceeded")),
		Math.max(1, deadlineAtMs - performance.now()),
	);
	try {
		for await (const chunk of process.stdin) {
			const buffer = Buffer.from(chunk);
			size += buffer.length;
			if (size > HOOK_RUNTIME_INPUT_MAX_BYTES) return null;
			chunks.push(buffer);
		}
	} catch {
		return null;
	} finally {
		clearTimeout(timeout);
	}
	return Buffer.concat(chunks).toString("utf8");
}

function isHookRuntimeWorkerData(value: unknown): value is HookRuntimeWorkerData {
	return (
		Boolean(value) &&
		typeof value === "object" &&
		(value as HookRuntimeWorkerData).role === "hook-runtime" &&
		typeof (value as HookRuntimeWorkerData).command === "string" &&
		typeof (value as HookRuntimeWorkerData).raw === "string" &&
		typeof (value as HookRuntimeWorkerData).deadlineAtMs === "number"
	);
}

async function runSupervisedHookRuntime(
	command: string,
	raw: string,
	deadlineAtMs: number,
): Promise<string> {
	const fallbackOutput = fallback(command);
	const remaining = Math.floor(deadlineAtMs - performance.now());
	if (remaining < 1) return fallbackOutput;
	return new Promise((resolveOutput) => {
		let worker: Worker;
		try {
			worker = new Worker(new URL(import.meta.url), {
				workerData: {
					role: "hook-runtime",
					command,
					raw,
					deadlineAtMs,
				} satisfies HookRuntimeWorkerData,
			});
		} catch {
			resolveOutput(fallbackOutput);
			return;
		}
		worker.unref();
		let settled = false;
		const finish = (output: string): void => {
			if (settled) return;
			settled = true;
			clearTimeout(timeout);
			void worker.terminate();
			resolveOutput(output);
		};
		const timeout = setTimeout(() => finish(fallbackOutput), remaining);
		worker.once("message", (message: unknown) => {
			if (
				typeof message !== "string" ||
				Buffer.byteLength(message, "utf8") > HOOK_RUNTIME_OUTPUT_MAX_BYTES
			) {
				finish(fallbackOutput);
				return;
			}
			finish(message);
		});
		worker.once("messageerror", () => finish(fallbackOutput));
		worker.once("error", () => finish(fallbackOutput));
		worker.once("exit", () => finish(fallbackOutput));
	});
}

async function main(): Promise<void> {
	const command = process.argv[2] ?? "";
	if (!COMMANDS.has(command)) {
		process.exitCode = 2;
		return;
	}
	// Leave process startup/output time inside the externally observed hard cap.
	const deadlineAtMs = clientHardCapMs(command) - 50;
	const raw = await readStdin(deadlineAtMs);
	const output =
		raw === null ? fallback(command) : await runSupervisedHookRuntime(command, raw, deadlineAtMs);
	if (output) {
		await new Promise<void>((resolveWrite) => {
			let settled = false;
			const finish = (): void => {
				if (settled) return;
				settled = true;
				clearTimeout(timeout);
				resolveWrite();
			};
			const timeout = setTimeout(
				() => {
					process.stdout.destroy();
					finish();
				},
				Math.max(1, deadlineAtMs - performance.now()),
			);
			process.stdout.write(output, finish);
		});
	}
}

if (!isMainThread && isHookRuntimeWorkerData(workerData)) {
	let output = fallback(workerData.command);
	try {
		output = await runHookRuntime(workerData.command, workerData.raw, workerData.deadlineAtMs);
	} catch {
		// The parent emits the exact fail-open result.
	}
	parentPort?.postMessage(output);
} else if (
	isMainThread &&
	process.argv[1] &&
	pathToFileURL(resolve(process.argv[1])).href === import.meta.url
) {
	await main();
}
