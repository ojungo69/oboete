import { randomUUID } from "node:crypto";
import { statSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, relative, resolve, sep } from "node:path";
import {
	type FileContextRetrievalAttempt,
	preprocessAdapterEvent,
	type RefQueryResult,
	resolveHookProject,
} from "@codemem/core";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import { addDbOption, type DbOpts, resolveDbOpt } from "../shared-options.js";
import { logHookEvent } from "./claude-hook-plugin-log.js";
import {
	deliverHookEvent,
	type PreparedHookEvent,
	prepareHookEvent,
	requestHookRpc,
} from "./hook-rpc-client.js";

type FileContextResult = {
	continue?: true;
	hookSpecificOutput?: {
		hookEventName: "PreToolUse";
		permissionDecision: "allow";
		additionalContext: string;
	};
};

type FileContextOpts = DbOpts & { deadlineAtMs?: number };

type FileContextDeps = {
	queryByFile?: typeof queryByFile;
	statFile?: typeof statFile;
	deliver?: typeof deliverHookEvent;
	recordAttempt?: (input: FileContextRetrievalAttempt) => Promise<void>;
	updateDelivery?: (attemptId: string, status: "handed_off" | "failed") => Promise<void>;
	now?: () => Date;
	createAttemptId?: () => string;
};

const FILE_GATE_MIN_BYTES = 1500;
const FETCH_LIMIT = 40;
const DISPLAY_LIMIT = 15;
// Mtime tolerance — observations from within ~5 minutes of the file's
// mtime are treated as fresh enough to surface without a staleness
// header. Smaller than this we'd false-positive on routine edits;
// larger and branch switches read clean.
const MTIME_FRESH_TOLERANCE_MS = 5 * 60 * 1000;

// Small but high-signal file extensions: configs and infra files are
// load-bearing for past decisions even at <1500 bytes, so bypass the
// size gate for them.
const SMALL_FILE_BYPASS_PATTERNS: RegExp[] = [
	/\.(json|jsonc|toml|ya?ml)$/i,
	/\.env(\.|$)/i,
	/(^|\/)dockerfile(\.|$)/i,
	/\.config\.(js|ts|mjs|cjs|json)$/i,
];

const KIND_ICONS: Record<string, string> = {
	decision: "⚖️",
	bugfix: "🔴",
	feature: "🟢",
	refactor: "🔄",
	discovery: "🔵",
	change: "✅",
	exploration: "🔬",
};

function emitJson(value: FileContextResult): void {
	console.log(JSON.stringify(value));
}

function continueResult(): FileContextResult {
	return { continue: true };
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

function expandHome(value: string): string {
	if (value === "~") return homedir();
	if (value.startsWith("~/")) return resolve(homedir(), value.slice(2));
	return value;
}

function extractFilePath(payload: Record<string, unknown>): string | null {
	const toolInput = payload.tool_input;
	if (!toolInput || typeof toolInput !== "object" || Array.isArray(toolInput)) {
		return null;
	}
	const filePath = (toolInput as Record<string, unknown>).file_path;
	return typeof filePath === "string" && filePath.trim() ? filePath.trim() : null;
}

type StatResult = { sizeBytes: number; mtimeMs: number } | null;

function statFile(absPath: string): StatResult {
	try {
		const stat = statSync(absPath);
		return { sizeBytes: stat.size, mtimeMs: stat.mtimeMs };
	} catch {
		return null;
	}
}

function parseJsonArray(value: string | null | undefined): string[] {
	if (!value) return [];
	try {
		const parsed = JSON.parse(value) as unknown;
		if (!Array.isArray(parsed)) return [];
		return parsed.filter((item): item is string => typeof item === "string");
	} catch {
		return [];
	}
}

function normalizePathForCompare(path: string): string {
	return path.replaceAll("\\", "/");
}

type ScoredObservation = { row: RefQueryResult; score: number; idx: number };

function scoreRow(row: RefQueryResult, normalizedTarget: string, idx: number): ScoredObservation {
	const filesModified = parseJsonArray(row.files_modified);
	const inModified = filesModified.some((f) => normalizePathForCompare(f) === normalizedTarget);

	// Specificity is "did this session focus on the target file?"
	// `files_read` is dominated by the agent crawling the repo for
	// context, so it shouldn't dilute that signal — score by
	// files_modified only.
	let score = 0;
	if (inModified) score += 2;
	if (filesModified.length <= 1) score += 2;
	else if (filesModified.length <= 3) score += 1;

	return { row, score, idx };
}

function scoreAndDedupe(
	rows: RefQueryResult[],
	targetPath: string,
	limit: number,
): RefQueryResult[] {
	const normalizedTarget = normalizePathForCompare(targetPath);
	const scored = rows.map((row, idx) => scoreRow(row, normalizedTarget, idx));

	// Dedupe by session keeping the highest-scoring observation per
	// session. Tiebreak on smaller idx — `findByFile` returns rows
	// ORDER BY created_at DESC so a smaller idx means more recent.
	const bestPerSession = new Map<number, ScoredObservation>();
	for (const item of scored) {
		const existing = bestPerSession.get(item.row.session_id);
		if (
			!existing ||
			item.score > existing.score ||
			(item.score === existing.score && item.idx < existing.idx)
		) {
			bestPerSession.set(item.row.session_id, item);
		}
	}

	const deduped = Array.from(bestPerSession.values());
	deduped.sort((a, b) => b.score - a.score || a.idx - b.idx);
	return deduped.slice(0, limit).map((s) => s.row);
}

function compactTime(timeStr: string): string {
	return timeStr.toLowerCase().replace(" am", "a").replace(" pm", "p");
}

function formatTime(epochMs: number): string {
	return new Date(epochMs).toLocaleString("en-US", {
		hour: "numeric",
		minute: "2-digit",
		hour12: true,
	});
}

function formatDate(epochMs: number): string {
	return new Date(epochMs).toLocaleString("en-US", {
		month: "short",
		day: "numeric",
		year: "numeric",
	});
}

function formatTimeline(
	rows: RefQueryResult[],
	filePath: string,
	staleness: { fileMtimeMs: number; newestObservationMs: number } | null,
): string {
	const safePath = filePath
		.replaceAll("\\", "\\\\")
		.replaceAll('"', String.raw`\"`)
		.replaceAll("\n", String.raw`\n`);
	const enriched = rows
		.map((row) => ({ row, epochMs: Date.parse(row.created_at) }))
		.filter((item) => Number.isFinite(item.epochMs) && item.epochMs > 0);

	const byDay = new Map<string, typeof enriched>();
	for (const item of enriched) {
		const day = formatDate(item.epochMs);
		const bucket = byDay.get(day);
		if (bucket) bucket.push(item);
		else byDay.set(day, [item]);
	}

	const sortedDays = Array.from(byDay.entries()).sort((a, b) => {
		const aMin = Math.min(...a[1].map((i) => i.epochMs));
		const bMin = Math.min(...b[1].map((i) => i.epochMs));
		return aMin - bMin;
	});

	const ids = rows.map((r) => r.id);
	const lines: string[] = [
		`This file (${safePath}) has prior codemem observations. The Read result below is unchanged.`,
		`- Fetch full bodies on demand: memory.get_observations([${ids.join(", ")}]).`,
	];
	if (staleness) {
		const driftMinutes = Math.max(
			1,
			Math.round((staleness.fileMtimeMs - staleness.newestObservationMs) / 60_000),
		);
		lines.unshift(
			`Heads up: this file was modified ~${driftMinutes} min after the most recent observation below. Past entries may be partially stale — verify against the Read result before relying on them.`,
		);
	}

	for (const [day, dayItems] of sortedDays) {
		const chronological = [...dayItems].sort((a, b) => a.epochMs - b.epochMs);
		lines.push(`### ${day}`);
		for (const { row, epochMs } of chronological) {
			const title = (row.title || "Untitled")
				.replace(/[\r\n\t]+/g, " ")
				.replace(/\s+/g, " ")
				.trim()
				.slice(0, 160);
			const icon = KIND_ICONS[row.kind] ?? "❔";
			const time = compactTime(formatTime(epochMs));
			lines.push(`${row.id} ${time} ${icon} (${row.kind}) ${title}`);
		}
	}

	return lines.join("\n");
}

async function queryByFile(
	relativePath: string,
	project: string | null,
	limit: number,
	deadlineAtMs?: number,
	dbPath?: string,
): Promise<RefQueryResult[]> {
	const result = await requestHookRpc(
		"claude",
		"POST /v1/search",
		{
			requestId: randomUUID(),
			mode: "find_by_file",
			repositoryPath: relativePath,
			limit,
			filters: project ? { project } : {},
		},
		{ deadlineAtMs, dbPath },
	);
	return Array.isArray(result.items) ? (result.items as RefQueryResult[]) : [];
}

function resolveProject(payload: Record<string, unknown>): string | null {
	const cwd = typeof payload.cwd === "string" ? payload.cwd : null;
	return resolveHookProject(cwd, payload.project);
}

export async function buildClaudeFileContext(
	payload: Record<string, unknown>,
	_opts: FileContextOpts,
	deps: FileContextDeps = {},
): Promise<FileContextResult> {
	if (envTruthy(process.env.CODEMEM_PLUGIN_IGNORE)) return continueResult();
	let prepared: PreparedHookEvent;
	try {
		prepared = prepareHookEvent("claude", payload, _opts.deadlineAtMs);
	} catch {
		return continueResult();
	}
	const dbPath = resolveDbOpt(_opts);
	const delivery = (deps.deliver ?? deliverHookEvent)("claude", payload, {
		prepared,
		dbPath,
	}).catch(() => ({
		via: "dropped" as const,
	}));
	const finish = async (result: FileContextResult): Promise<FileContextResult> => {
		await delivery;
		return result;
	};
	if (prepared.status === "skipped") return finish(continueResult());
	const filePath = extractFilePath(payload);
	if (!filePath) return finish(continueResult());

	const now = deps.now ?? (() => new Date());
	const startedAt = now();
	const attemptId = (deps.createAttemptId ?? randomUUID)();
	const nativeSessionId = prepared.event.nativeSessionId;
	const sourceSessionId =
		typeof nativeSessionId === "string" && nativeSessionId.trim() ? nativeSessionId.trim() : null;
	const record = async (
		input: Omit<FileContextRetrievalAttempt, "attemptId" | "startedAt" | "completedAt">,
	): Promise<void> => {
		if (!envNotDisabled(process.env.CODEMEM_RETRIEVAL_LEDGER || "1")) return;
		try {
			const completedAt = now();
			const attempt: FileContextRetrievalAttempt = {
				...input,
				attemptId,
				startedAt: startedAt.toISOString(),
				completedAt: completedAt.toISOString(),
				sourceSessionId,
			};
			if (deps.recordAttempt) await deps.recordAttempt(attempt);
			else {
				await requestHookRpc(
					"claude",
					"POST /v1/retrieval/file-context",
					{ ...attempt },
					{
						deadlineAtMs: prepared.deadlineAtMs,
						dbPath,
					},
				);
			}
		} catch {
			// Retrieval attribution must not affect hook output.
		}
	};
	const updateDelivery = async (status: "handed_off" | "failed"): Promise<void> => {
		if (!envNotDisabled(process.env.CODEMEM_RETRIEVAL_LEDGER || "1")) return;
		try {
			if (deps.updateDelivery) await deps.updateDelivery(attemptId, status);
			else {
				await requestHookRpc(
					"claude",
					"POST /v1/retrieval/file-context/delivery",
					{ attemptId, status },
					{ deadlineAtMs: prepared.deadlineAtMs, dbPath },
				);
			}
		} catch {
			// Retrieval attribution must not affect hook output.
		}
	};
	if (!envNotDisabled(process.env.CODEMEM_FILE_CONTEXT || "1")) {
		await record({
			retrievalStatus: "skipped",
			failureCode: "file_context_disabled",
			failureStage: "configuration",
		});
		return finish(continueResult());
	}
	if (
		prepared.redaction.sensitivity !== "normal" ||
		prepared.redaction.private_content_omitted ||
		prepared.redaction.local_only
	) {
		await record({
			retrievalStatus: "skipped",
			failureCode: "privacy_policy",
			failureStage: "redaction",
		});
		return finish(continueResult());
	}

	const cwd = typeof payload.cwd === "string" && payload.cwd.trim() ? payload.cwd : process.cwd();
	const expandedPath = expandHome(filePath);
	const absolutePath = isAbsolute(expandedPath) ? expandedPath : resolve(cwd, expandedPath);
	const relativePath = relative(cwd, absolutePath).split(sep).join("/");

	// Reject paths that escape cwd. `startsWith("..")` would also reject
	// in-repo basenames that begin with `..` (e.g. `..hidden`); a
	// segment-aware check distinguishes those from the `../foo` parent
	// traversal. `isAbsolute` catches the Windows cross-drive case where
	// `relative('C:\\repo', 'D:\\x')` returns an absolute-shaped string.
	const escapesCwd =
		relativePath === ".." || relativePath.startsWith("../") || isAbsolute(relativePath);
	if (!relativePath || escapesCwd) {
		logHookEvent("file_context.skip reason=outside_cwd");
		await record({
			retrievalStatus: "skipped",
			failureCode: "outside_cwd",
			failureStage: "path_validation",
		});
		return finish(continueResult());
	}

	const project = resolveProject(payload);
	const redactedQuery = preprocessAdapterEvent(
		{ repositoryPath: relativePath, project },
		{ allowlist: ["repositoryPath", "project"], config: prepared.config },
	);
	const safePath = redactedQuery.payload.repositoryPath;
	const safeProject = redactedQuery.payload.project;
	if (
		redactedQuery.sensitivity !== "normal" ||
		redactedQuery.private_content_omitted ||
		typeof safePath !== "string" ||
		safePath !== relativePath ||
		(project !== null && safeProject !== project)
	) {
		await record({
			retrievalStatus: "skipped",
			failureCode: "privacy_policy",
			failureStage: "redaction",
		});
		return finish(continueResult());
	}
	const safeProjectValue = typeof safeProject === "string" ? safeProject : null;

	const minBytes = Number.parseInt(
		process.env.CODEMEM_FILE_CONTEXT_MIN_BYTES ?? `${FILE_GATE_MIN_BYTES}`,
		10,
	);
	const minBytesEffective =
		Number.isFinite(minBytes) && minBytes >= 0 ? minBytes : FILE_GATE_MIN_BYTES;

	const stat = (deps.statFile ?? statFile)(absolutePath);
	if (!stat) {
		logHookEvent(`file_context.skip reason=stat_failed path=${JSON.stringify(safePath)}`);
		await record({
			retrievalStatus: "skipped",
			failureCode: "stat_failed",
			failureStage: "file_access",
			repositoryPath: safePath,
		});
		return finish(continueResult());
	}
	const bypassSizeGate = SMALL_FILE_BYPASS_PATTERNS.some((p) => p.test(safePath));
	if (stat.sizeBytes < minBytesEffective && !bypassSizeGate) {
		logHookEvent(
			`file_context.skip reason=below_size_gate path=${JSON.stringify(safePath)} size=${stat.sizeBytes} gate=${minBytesEffective}`,
		);
		await record({
			retrievalStatus: "skipped",
			failureCode: "below_size_gate",
			failureStage: "size_gate",
			repositoryPath: safePath,
		});
		return finish(continueResult());
	}

	const queryFn = deps.queryByFile ?? queryByFile;
	let rows: RefQueryResult[] = [];
	try {
		rows = await queryFn(safePath, safeProjectValue, FETCH_LIMIT, prepared.deadlineAtMs, dbPath);
	} catch {
		logHookEvent("codemem claude-hook-file-context query failed");
		await record({
			retrievalStatus: "failed",
			failureCode: "query_failed",
			failureStage: "retrieval",
			project: safeProjectValue,
			repositoryPath: safePath,
		});
		return finish(continueResult());
	}

	if (rows.length === 0) {
		logHookEvent(
			`file_context.skip reason=no_observations path=${JSON.stringify(safePath)} project=${JSON.stringify(safeProjectValue ?? "")}`,
		);
		await record({
			retrievalStatus: "no_results",
			project: safeProjectValue,
			repositoryPath: safePath,
		});
		return finish(continueResult());
	}

	const top = scoreAndDedupe(rows, safePath, DISPLAY_LIMIT);
	if (top.length === 0) {
		logHookEvent(
			`file_context.skip reason=no_top_after_dedupe path=${JSON.stringify(safePath)} candidates=${rows.length}`,
		);
		await record({
			retrievalStatus: "succeeded",
			candidateIds: rows.map((row) => row.id),
			candidateCount: rows.length,
			selectedIds: [],
			failureCode: "no_top_after_dedupe",
			failureStage: "selection",
			project: safeProjectValue,
			repositoryPath: safePath,
		});
		return finish(continueResult());
	}

	let staleness: { fileMtimeMs: number; newestObservationMs: number } | null = null;
	if (stat.mtimeMs > 0) {
		const newestObservationMs = top.reduce((max, row) => {
			const epoch = Date.parse(row.created_at);
			return Number.isFinite(epoch) && epoch > max ? epoch : max;
		}, 0);
		if (newestObservationMs > 0 && stat.mtimeMs > newestObservationMs + MTIME_FRESH_TOLERANCE_MS) {
			staleness = { fileMtimeMs: stat.mtimeMs, newestObservationMs };
		}
	}

	await record({
		retrievalStatus: "succeeded",
		candidateIds: rows.map((row) => row.id),
		candidateCount: rows.length,
		selectedIds: top.map((row) => row.id),
		project: safeProjectValue,
		repositoryPath: safePath,
	});

	let timeline: string;
	try {
		timeline = formatTimeline(top, safePath, staleness);
	} catch {
		await updateDelivery("failed");
		return finish(continueResult());
	}

	logHookEvent(
		`file_context.ok path=${JSON.stringify(safePath)} candidates=${rows.length} surfaced=${top.length} project=${JSON.stringify(safeProjectValue ?? "")} stale=${staleness ? "true" : "false"}`,
	);
	await updateDelivery("handed_off");
	return finish({
		hookSpecificOutput: {
			hookEventName: "PreToolUse",
			permissionDecision: "allow",
			additionalContext: timeline,
		},
	});
}

const claudeHookFileContextCmd = new Command("claude-hook-file-context")
	.configureHelp(helpStyle)
	.description(
		"Return Claude PreToolUse:Read additionalContext from per-file observation timeline",
	);

addDbOption(claudeHookFileContextCmd);

export const claudeHookFileContextCommand = claudeHookFileContextCmd.action(
	async (opts: FileContextOpts) => {
		let raw = "";
		for await (const chunk of process.stdin) {
			raw += String(chunk);
		}
		const trimmed = raw.trim();
		if (!trimmed) {
			emitJson(continueResult());
			return;
		}

		try {
			const parsed = JSON.parse(trimmed) as unknown;
			if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error();
			emitJson(await buildClaudeFileContext(parsed as Record<string, unknown>, opts));
		} catch {
			emitJson(continueResult());
		}
	},
);

export type { FileContextResult };
