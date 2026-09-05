import {
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
	HOOK_DELIVERY_BUDGETS,
	parseAgentMemoryToml,
	readCurrentDatabasePointer,
	resolveRuntimeDataDir,
	resolveSpoolLayout,
	resolveStorageLayout,
	startDaemon,
} from "@codemem/core";
import { afterEach, describe, expect, it } from "vitest";
import { openTestMemoryStore } from "../../../core/src/test-utils.js";
import { buildClaudeFileContext } from "./claude-hook-file-context.js";
import { ingestClaudeHookPayload } from "./claude-hook-ingest.js";
import { buildClaudeHookInjection } from "./claude-hook-inject.js";
import { statePathForSession } from "./claude-hook-session-state.js";
import { buildCodexHookInjection } from "./codex-hook-inject.js";
import {
	deliverHookEvent,
	prepareHookEvent,
	requestHookPack,
	requestHookRpc,
} from "./hook-rpc-client.js";

const roots: string[] = [];
const originalDataDir = process.env.CODEMEM_DATA_DIR;
const originalDb = process.env.CODEMEM_DB;
const originalContextDir = process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR;

afterEach(async () => {
	if (originalDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
	else process.env.CODEMEM_DATA_DIR = originalDataDir;
	if (originalDb === undefined) delete process.env.CODEMEM_DB;
	else process.env.CODEMEM_DB = originalDb;
	if (originalContextDir === undefined) delete process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR;
	else process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = originalContextDir;
	await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function tempRoot(): Promise<string> {
	const root = await mkdtemp(join(tmpdir(), "codemem-hook-thin-"));
	roots.push(root);
	return root;
}

describe("hook thin clients", () => {
	it("uses the declared Agent-specific outer watchdogs", () => {
		const hooks = (path: string): Array<{ command: string; timeout: number }> => {
			const config = JSON.parse(readFileSync(path, "utf8")) as {
				hooks: Record<string, Array<{ hooks: Array<{ command: string; timeout?: number }> }>>;
			};
			return Object.values(config.hooks).flatMap((groups) =>
				groups.flatMap((group) =>
					group.hooks.map((hook) => ({ command: hook.command, timeout: hook.timeout ?? 0 })),
				),
			);
		};
		const claude = hooks(join(process.cwd(), "plugins", "claude", "hooks", "hooks.json"));
		const codex = hooks(join(process.cwd(), "plugins", "codex", "hooks", "hooks.json"));
		expect(new Set(claude.map((hook) => hook.timeout))).toEqual(
			new Set([HOOK_DELIVERY_BUDGETS.claude.outerWatchdogMs / 1000]),
		);
		expect(new Set(codex.map((hook) => hook.timeout))).toEqual(
			new Set([HOOK_DELIVERY_BUDGETS.codex.outerWatchdogMs / 1000]),
		);
		expect([...claude, ...codex].every((hook) => hook.command.includes("hook-runtime.mjs"))).toBe(
			true,
		);
		expect(
			existsSync(join(process.cwd(), "plugins", "claude", "scripts", "hook-runtime.mjs")),
		).toBe(true);
		expect(existsSync(join(process.cwd(), "plugins", "codex", "scripts", "hook-runtime.mjs"))).toBe(
			true,
		);
	});

	it("P1-T041-01-hook-timeout-rescue preserves a boundary event after RPC cutoff", async () => {
		const root = await tempRoot();
		const dataDir = join(root, "data");
		const storage = resolveStorageLayout(dataDir);
		mkdirSync(storage.controlDir, { recursive: true, mode: 0o700 });
		const server = createServer((socket) => socket.on("data", () => {}));
		await new Promise<void>((resolveListen, reject) => {
			server.once("error", reject);
			server.listen(storage.socketPath, resolveListen);
		});

		try {
			const started = performance.now();
			const result = await deliverHookEvent(
				"claude",
				{
					hook_event_name: "SessionEnd",
					session_id: "session-timeout",
					cwd: "/tmp/project",
					timestamp: "2026-08-14T01:00:00.000Z",
					reason: "<private>local reason</private>",
				},
				{ dataDir, rpcTimeoutMs: 25 },
			);
			expect(performance.now() - started).toBeLessThan(500);
			expect(result.via).toBe("spool");

			const spool = resolveSpoolLayout(dataDir);
			const ready = readdirSync(spool.readyDir);
			expect(ready).toHaveLength(1);
			const entry = JSON.parse(readFileSync(join(spool.readyDir, ready[0] as string), "utf8")) as {
				quotaClass: string;
				body: { event: { kind: string; payload: unknown } };
			};
			expect(entry.quotaClass).toBe("reserved");
			expect(entry.body.event.kind).toBe("session_ended");
			expect(JSON.stringify(entry.body.event.payload)).not.toContain("local reason");
		} finally {
			await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
		}

		delete process.env.CODEMEM_DATA_DIR;
		const customDbPath = join(root, "hook.sqlite");
		process.env.CODEMEM_DB = customDbPath;
		const customDataDir = resolveRuntimeDataDir({ dbPath: customDbPath });
		roots.push(customDataDir);
		const customStorage = resolveStorageLayout(customDataDir);
		mkdirSync(customStorage.controlDir, { recursive: true, mode: 0o700 });
		const rpcServer = createServer((socket) => {
			socket.once("data", (chunk: Buffer) => {
				const request = JSON.parse(chunk.toString("utf8")) as { id: string };
				socket.end(`${JSON.stringify({ id: request.id, result: { status: "ok" } })}\n`);
			});
		});
		await new Promise<void>((resolveListen, reject) => {
			rpcServer.once("error", reject);
			rpcServer.listen(customStorage.socketPath, resolveListen);
		});
		try {
			await expect(requestHookRpc("claude", "GET /v1/health", {})).resolves.toEqual({
				status: "ok",
			});
		} finally {
			await new Promise<void>((resolveClose) => rpcServer.close(() => resolveClose()));
		}

		const envResult = await deliverHookEvent(
			"claude",
			{
				hook_event_name: "SessionEnd",
				session_id: "session-env-db",
				cwd: root,
				timestamp: "2026-08-14T01:00:00.000Z",
				reason: "done",
			},
			{ rpcTimeoutMs: 25 },
		);
		expect(envResult).toEqual({ via: "spool" });
		expect(readdirSync(resolveSpoolLayout(customDataDir).readyDir)).toHaveLength(1);

		delete process.env.CODEMEM_DB;
		const optionDbPath = join(root, "hook-option.sqlite");
		const optionDataDir = resolveRuntimeDataDir({ dbPath: optionDbPath });
		roots.push(optionDataDir);
		const optionResult = await deliverHookEvent(
			"claude",
			{
				hook_event_name: "SessionEnd",
				session_id: "session-option-db",
				cwd: root,
				timestamp: "2026-08-14T01:00:00.000Z",
				reason: "done",
			},
			{ dbPath: optionDbPath, rpcTimeoutMs: 25 },
		);
		expect(optionResult).toEqual({ via: "spool" });
		expect(readdirSync(resolveSpoolLayout(optionDataDir).readyDir)).toHaveLength(1);
	});

	it("does not start a spool write after the fsync reserve is exhausted", async () => {
		const root = await tempRoot();
		const dataDir = join(root, "data");
		const payload = {
			hook_event_name: "SessionEnd",
			session_id: "session-expired-deadline",
			cwd: root,
			timestamp: "2026-08-14T01:00:00.000Z",
			reason: "done",
		};
		const prepared = prepareHookEvent("claude", payload);
		expect(prepared.status).toBe("ready");
		prepared.deadlineAtMs = performance.now() - 1;

		await expect(deliverHookEvent("claude", payload, { dataDir, prepared })).resolves.toEqual({
			via: "dropped",
		});
		const readyDir = resolveSpoolLayout(dataDir).readyDir;
		expect(existsSync(readyDir) ? readdirSync(readyDir) : []).toEqual([]);
	});

	it.each([
		"claude",
		"codex",
	] as const)("keeps the %s default RPC cutoff inside its hard cap and spools", async (agent) => {
		const root = await tempRoot();
		const dataDir = join(root, "data");
		const storage = resolveStorageLayout(dataDir);
		mkdirSync(storage.controlDir, { recursive: true, mode: 0o700 });
		const server = createServer((socket) => socket.on("data", () => {}));
		await new Promise<void>((resolveListen, reject) => {
			server.once("error", reject);
			server.listen(storage.socketPath, resolveListen);
		});

		try {
			const started = performance.now();
			const result = await deliverHookEvent(
				agent,
				{
					hook_event_name: "UserPromptSubmit",
					session_id: `${agent}-default-deadline`,
					cwd: root,
					timestamp: "2026-08-14T01:00:00.000Z",
					prompt: "preserve this event",
				},
				{ dataDir },
			);
			const elapsed = performance.now() - started;
			expect(result).toEqual({ via: "spool" });
			expect(elapsed).toBeGreaterThanOrEqual(HOOK_DELIVERY_BUDGETS[agent].rpcCutoffMs - 200);
			expect(elapsed).toBeLessThan(HOOK_DELIVERY_BUDGETS[agent].clientHardCapMs + 250);
		} finally {
			await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
		}
	});

	it("P1-T041-01b redacts hook content before the RPC write", async () => {
		const root = await tempRoot();
		const dataDir = join(root, "data");
		const storage = resolveStorageLayout(dataDir);
		mkdirSync(storage.controlDir, { recursive: true, mode: 0o700 });
		let request = "";
		const server = createServer((socket) => {
			socket.on("data", (chunk: Buffer) => {
				request += chunk.toString("utf8");
				const newline = request.indexOf("\n");
				if (newline < 0) return;
				const parsed = JSON.parse(request.slice(0, newline)) as { id: string };
				socket.end(`${JSON.stringify({ id: parsed.id, result: { status: "committed" } })}\n`);
			});
		});
		await new Promise<void>((resolveListen, reject) => {
			server.once("error", reject);
			server.listen(storage.socketPath, resolveListen);
		});

		const secret = `ghp_${"A".repeat(36)}`;
		try {
			await expect(
				deliverHookEvent(
					"claude",
					{
						hook_event_name: "UserPromptSubmit",
						session_id: "session-redaction",
						cwd: "/tmp/project",
						timestamp: "2026-08-14T01:00:00.000Z",
						prompt: `credential ${secret} <private>hidden</private> <local-only>device</local-only>`,
					},
					{ dataDir },
				),
			).resolves.toEqual({ via: "rpc" });
			expect(request).not.toContain(secret);
			expect(request).not.toContain("hidden");
			const rpc = JSON.parse(request.trim()) as {
				body: {
					adapterRedaction: Record<string, unknown>;
					event: { payload: unknown; sensitivity: string };
				};
			};
			expect(rpc.body.event).toMatchObject({ payload: {}, sensitivity: "secret" });
			expect(rpc.body.adapterRedaction).toMatchObject({
				sensitivity: "secret",
				private_content_omitted: true,
				local_only: true,
			});
		} finally {
			await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
		}
	});

	it("P1-T041-01c applies repository policy before RPC and spool", async () => {
		const root = await tempRoot();
		const workspace = join(root, "workspace");
		const dataDir = join(root, "data");
		mkdirSync(join(workspace, ".git"), { recursive: true });
		writeFileSync(
			join(workspace, ".agent-memory.toml"),
			[
				'secret_regex = ["ACME-[0-9]{8}"]',
				'private_regex = ["customer-[0-9]+"]',
				'tool_field_allowlist = ["file_path"]',
				'tool_field_denylist = ["debug_blob"]',
			].join("\n"),
		);
		const storage = resolveStorageLayout(dataDir);
		mkdirSync(storage.controlDir, { recursive: true, mode: 0o700 });
		let request = "";
		const server = createServer((socket) => {
			socket.on("data", (chunk: Buffer) => {
				request += chunk.toString("utf8");
				const parsed = JSON.parse(request.trim()) as { id: string };
				socket.end(`${JSON.stringify({ id: parsed.id, result: { status: "committed" } })}\n`);
			});
		});
		await new Promise<void>((resolveListen, reject) => {
			server.once("error", reject);
			server.listen(storage.socketPath, resolveListen);
		});

		const secret = "ACME-12345678";
		const privateValue = "customer-9876";
		process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = join(root, "session-state");
		const payload = {
			hook_event_name: "UserPromptSubmit",
			session_id: "session-project-policy-rpc",
			cwd: workspace,
			timestamp: "2026-08-14T01:00:00.000Z",
			prompt: `keep ${secret} ${privateValue} <private>hidden</private>`,
		};
		try {
			await expect(deliverHookEvent("claude", payload, { dataDir })).resolves.toEqual({
				via: "rpc",
			});
			expect(request).not.toContain(secret);
			expect(request).not.toContain(privateValue);
			expect(request).not.toContain("hidden");
		} finally {
			await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
		}

		await expect(
			deliverHookEvent(
				"claude",
				{ ...payload, session_id: "session-project-policy-spool" },
				{ dataDir, rpcTimeoutMs: 25 },
			),
		).resolves.toEqual({ via: "spool" });
		const spool = resolveSpoolLayout(dataDir);
		const persisted = readdirSync(spool.readyDir)
			.map((name) => readFileSync(join(spool.readyDir, name), "utf8"))
			.join("\n");
		expect(persisted).not.toContain(secret);
		expect(persisted).not.toContain(privateValue);
		expect(persisted).not.toContain("hidden");
		expect(JSON.parse(persisted)).toMatchObject({
			redaction: { sensitivity: "secret", private_content_omitted: true },
		});

		const fieldPolicyPayload = {
			hook_event_name: "PostToolUse",
			session_id: "session-tool-field-policy",
			cwd: workspace,
			timestamp: "2026-08-14T01:00:00.000Z",
			tool_name: "Edit",
			tool_input: {
				file_path: "src/public.ts",
				path: "src/unlisted.ts",
				debug_blob: "drop",
			},
			tool_response: "ok",
		};
		const fieldPolicyEvent = prepareHookEvent("claude", fieldPolicyPayload);
		expect(fieldPolicyEvent.status).toBe("ready");
		if (fieldPolicyEvent.status !== "ready") throw new Error("expected a ready event");
		const persistedEvent = JSON.stringify(fieldPolicyEvent.event);
		expect(persistedEvent).toContain("src/public.ts");
		expect(persistedEvent).not.toContain("src/unlisted.ts");
		expect(persistedEvent).not.toContain("debug_blob");
		await ingestClaudeHookPayload(
			fieldPolicyPayload,
			{ host: "127.0.0.1", port: 38888 },
			{ deliver: async () => ({ via: "rpc" }) },
		);
		const fieldPolicyState = readFileSync(
			statePathForSession(fieldPolicyPayload.session_id),
			"utf8",
		);
		expect(fieldPolicyState).toContain("src/public.ts");
		expect(fieldPolicyState).not.toContain("src/unlisted.ts");
		expect(fieldPolicyState).not.toContain("debug_blob");

		const stateSession = "session-private-file-state";
		await ingestClaudeHookPayload(
			{
				hook_event_name: "PostToolUse",
				session_id: stateSession,
				cwd: workspace,
				timestamp: "2026-08-14T01:00:00.000Z",
				tool_name: "Edit",
				tool_input: { file_path: `src/${privateValue}.ts` },
				tool_response: "ok",
			},
			{ host: "127.0.0.1", port: 38888 },
			{ deliver: async () => ({ via: "rpc" }) },
		);
		expect(existsSync(statePathForSession(stateSession))).toBe(false);

		const privateSessionId = `session-${privateValue}-stable`;
		await ingestClaudeHookPayload(
			{
				hook_event_name: "UserPromptSubmit",
				session_id: privateSessionId,
				cwd: workspace,
				timestamp: "2026-08-14T01:00:00.000Z",
				prompt: "visible prompt",
			},
			{ host: "127.0.0.1", port: 38888 },
			{ deliver: async () => ({ via: "rpc" }) },
		);
		expect(existsSync(statePathForSession(privateSessionId))).toBe(false);
		expect(existsSync(statePathForSession("session--stable"))).toBe(true);
		expect(readdirSync(join(root, "session-state")).join("\n")).not.toContain(privateValue);
	});

	it("P1-T041-01d shares the sanitized prompt with pack RPC and session state", async () => {
		const root = await tempRoot();
		const workspace = join(root, "workspace");
		const dataDir = join(root, "data");
		mkdirSync(join(workspace, ".git"), { recursive: true });
		writeFileSync(join(workspace, ".agent-memory.toml"), 'private_regex = ["customer-[0-9]+"]\n');
		process.env.CODEMEM_DATA_DIR = dataDir;
		process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = join(root, "session-state");
		const storage = resolveStorageLayout(dataDir);
		mkdirSync(storage.controlDir, { recursive: true, mode: 0o700 });
		const requests: string[] = [];
		const server = createServer((socket) => {
			let request = "";
			socket.on("data", (chunk: Buffer) => {
				request += chunk.toString("utf8");
				if (!request.includes("\n")) return;
				requests.push(request);
				const parsed = JSON.parse(request.trim()) as { id: string; method: string };
				const result =
					parsed.method === "POST /v1/context/pack"
						? { pack: { pack_text: "", items: [], metrics: { pack_tokens: 0 } } }
						: { status: "committed" };
				socket.end(`${JSON.stringify({ id: parsed.id, result })}\n`);
			});
		});
		await new Promise<void>((resolveListen, reject) => {
			server.once("error", reject);
			server.listen(storage.socketPath, resolveListen);
		});

		const sessionId = "session-safe-query";
		const privateValue = "customer-2468";
		try {
			await buildClaudeHookInjection(
				{
					hook_event_name: "UserPromptSubmit",
					session_id: sessionId,
					cwd: workspace,
					timestamp: "2026-08-14T01:00:00.000Z",
					prompt: `resume ${privateValue} <private>hidden</private> visible`,
				},
				{},
			);
		} finally {
			await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
		}
		const wire = requests.join("\n");
		expect(requests).toHaveLength(2);
		expect(wire).toContain("visible");
		expect(wire).not.toContain(privateValue);
		expect(wire).not.toContain("hidden");
		const state = readFileSync(statePathForSession(sessionId), "utf8");
		expect(state).toContain("visible");
		expect(state).not.toContain(privateValue);
		expect(state).not.toContain("hidden");
	});

	it("redacts project and working-set filters before pack RPC", async () => {
		const root = await tempRoot();
		const dataDir = join(root, "data");
		const storage = resolveStorageLayout(dataDir);
		mkdirSync(storage.controlDir, { recursive: true, mode: 0o700 });
		let request = "";
		const server = createServer((socket) => {
			socket.on("data", (chunk: Buffer) => {
				request += chunk.toString("utf8");
				if (!request.includes("\n")) return;
				const parsed = JSON.parse(request.trim()) as { id: string };
				socket.end(
					`${JSON.stringify({ id: parsed.id, result: { pack: { pack_text: "", items: [] } } })}\n`,
				);
			});
		});
		await new Promise<void>((resolveListen, reject) => {
			server.once("error", reject);
			server.listen(storage.socketPath, resolveListen);
		});

		try {
			await requestHookPack(
				"claude",
				{
					context: "visible query",
					project: "customer-123",
					workingSetPaths: ["src/customer-123.ts", "src/public.ts"],
					limit: 8,
					tokenBudget: 800,
				},
				{
					dataDir,
					config: parseAgentMemoryToml('private_regex = ["customer-[0-9]+"]'),
				},
			);
		} finally {
			await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
		}
		expect(request).not.toContain("customer-123");
		const rpc = JSON.parse(request.trim()) as {
			body: { filters: { project?: string; working_set_paths?: string[] } };
		};
		expect(rpc.body.filters.project).toBeUndefined();
		expect(rpc.body.filters.working_set_paths).toEqual(["src/public.ts"]);
	});

	it("P1-T041-01e enforces ignore and local-only path policy", async () => {
		const root = await tempRoot();
		const workspace = join(root, "workspace");
		const nestedCwd = join(workspace, "packages", "app");
		const dataDir = join(root, "data");
		mkdirSync(nestedCwd, { recursive: true });
		mkdirSync(join(workspace, ".git"), { recursive: true });
		writeFileSync(
			join(workspace, ".agent-memory.toml"),
			[
				'ignore_paths = ["ignored/**", "packages/app/ignored/**"]',
				'local_only_paths = ["local/**", "packages/app/local/**"]',
			].join("\n"),
		);
		mkdirSync(join(workspace, "ignored"), { recursive: true });
		writeFileSync(join(workspace, "ignored", "secret.ts"), "private fixture");
		symlinkSync("ignored", join(workspace, "alias"), "dir");
		const base = {
			hook_event_name: "PreToolUse",
			session_id: "session-path-policy",
			cwd: workspace,
			timestamp: "2026-08-14T01:00:00.000Z",
			tool_name: "Read",
		};
		expect(
			prepareHookEvent("claude", {
				...base,
				tool_input: { file_path: "alias/secret.ts" },
			}).status,
		).toBe("skipped");
		expect(
			prepareHookEvent("claude", {
				...base,
				tool_name: "Bash",
				tool_input: { command: "cat ignored/secret.ts" },
			}).status,
		).toBe("skipped");
		let fileContextQueried = false;
		await buildClaudeFileContext(
			{ ...base, tool_input: { file_path: join(workspace, "ignored", "secret.ts") } },
			{},
			{
				queryByFile: async () => {
					fileContextQueried = true;
					return [];
				},
			},
		);
		expect(fileContextQueried).toBe(false);
		await buildClaudeFileContext(
			{
				...base,
				tool_use_id: "local-file-context",
				tool_input: { file_path: "local/notes.ts" },
			},
			{},
			{
				deliver: async () => ({ via: "rpc" }),
				recordAttempt: async () => {},
				statFile: () => ({ sizeBytes: 2_000, mtimeMs: 0 }),
				queryByFile: async () => {
					fileContextQueried = true;
					return [];
				},
			},
		);
		expect(fileContextQueried).toBe(false);
		process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = join(root, "session-state");
		const localStateSession = "session-local-state";
		await ingestClaudeHookPayload(
			{
				hook_event_name: "PostToolUse",
				session_id: localStateSession,
				cwd: workspace,
				timestamp: "2026-08-14T01:00:00.000Z",
				tool_name: "Edit",
				tool_input: { file_path: "local/notes.ts" },
				tool_response: "ok",
			},
			{ host: "127.0.0.1", port: 38888 },
			{ deliver: async () => ({ via: "rpc" }) },
		);
		expect(existsSync(statePathForSession(localStateSession))).toBe(false);
		const localPromptSession = "session-local-prompt";
		await buildClaudeHookInjection(
			{
				hook_event_name: "UserPromptSubmit",
				session_id: localPromptSession,
				cwd: workspace,
				timestamp: "2026-08-14T01:00:00.000Z",
				prompt: "resume <local-only>device-only detail</local-only>",
			},
			{},
			{ deliver: async () => ({ via: "rpc" }) },
		);
		expect(existsSync(statePathForSession(localPromptSession))).toBe(false);
		await expect(
			requestHookPack(
				"claude",
				{
					context: "resume <local-only>device-only detail</local-only>",
					project: null,
					limit: 8,
					tokenBudget: 800,
				},
				{ dataDir },
			),
		).resolves.toEqual({ packText: "", items: 0, packTokens: 0 });
		await expect(
			deliverHookEvent(
				"claude",
				{ ...base, tool_input: { file_path: join(workspace, "ignored", "secret.ts") } },
				{ dataDir, rpcTimeoutMs: 25 },
			),
		).resolves.toEqual({ via: "skipped" });
		await expect(
			deliverHookEvent(
				"claude",
				{ ...base, tool_use_id: "local-read", tool_input: { file_path: "local/notes.ts" } },
				{ dataDir, rpcTimeoutMs: 25 },
			),
		).resolves.toEqual({ via: "spool" });
		const spool = resolveSpoolLayout(dataDir);
		const persisted = JSON.parse(
			readFileSync(join(spool.readyDir, readdirSync(spool.readyDir)[0] as string), "utf8"),
		) as { redaction: { local_only: boolean } };
		expect(persisted.redaction.local_only).toBe(true);

		expect(
			prepareHookEvent("claude", {
				...base,
				tool_name: "apply_patch",
				tool_input: { patchText: "*** Update File: ignored/secret.ts" },
			}).status,
		).toBe("skipped");
		const localPatch = prepareHookEvent("claude", {
			...base,
			tool_name: "apply_patch",
			tool_input: { patchText: "*** Update File: local/notes.ts" },
		});
		expect(localPatch.status).toBe("ready");
		if (localPatch.status !== "ready") throw new Error("expected a ready event");
		expect(localPatch.redaction.local_only).toBe(true);

		expect(
			prepareHookEvent("claude", {
				...base,
				cwd: nestedCwd,
				tool_input: { file_path: "ignored/secret.ts" },
			}).status,
		).toBe("skipped");
		const nestedLocal = prepareHookEvent("claude", {
			...base,
			cwd: nestedCwd,
			tool_input: { file_path: "local/notes.ts" },
		});
		expect(nestedLocal.status).toBe("ready");
		if (nestedLocal.status !== "ready") throw new Error("expected a ready nested event");
		expect(nestedLocal.redaction.local_only).toBe(true);
	});

	it("P1-T041-02-inject-fail-open keeps both agents running without opening SQLite", async () => {
		const root = await tempRoot();
		process.env.CODEMEM_DATA_DIR = join(root, "missing-daemon");
		const forbiddenDb = join(root, "hook-client-must-not-open.sqlite");
		const payload = {
			hook_event_name: "UserPromptSubmit",
			session_id: "session-inject",
			cwd: root,
			prompt: "resume the task",
		};

		const started = performance.now();
		const [claude, codex] = await Promise.all([
			buildClaudeHookInjection(payload, { dbPath: forbiddenDb }),
			buildCodexHookInjection(payload, { dbPath: forbiddenDb }),
		]);
		expect(performance.now() - started).toBeLessThan(500);
		expect(claude).toEqual({ continue: true });
		expect(codex).toEqual({ continue: true });
		expect(existsSync(forbiddenDb)).toBe(false);
	});

	it("P1-T041-03-file-context-ledger writes the retrieval attempt and hook event through RPC", async () => {
		const root = await tempRoot();
		const dataDir = join(root, "data");
		const workspace = join(root, "workspace");
		const file = join(workspace, "src", "large.ts");
		mkdirSync(join(workspace, "src"), { recursive: true });
		writeFileSync(file, "x".repeat(2_000));
		process.env.CODEMEM_DATA_DIR = dataDir;
		const daemon = await startDaemon({ dataDir });
		const forbiddenDb = join(root, "file-context-must-not-open.sqlite");

		try {
			await expect(
				buildClaudeFileContext(
					{
						hook_event_name: "PreToolUse",
						session_id: "session-file-context",
						tool_use_id: "tool-read-1",
						tool_name: "Read",
						tool_input: { file_path: file },
						cwd: workspace,
						timestamp: "2026-08-14T01:00:00.000Z",
					},
					{ dbPath: forbiddenDb },
				),
			).resolves.toEqual({ continue: true });
			expect(existsSync(forbiddenDb)).toBe(false);
		} finally {
			await daemon.stop();
		}

		const layout = resolveStorageLayout(dataDir);
		const pointer = readCurrentDatabasePointer(layout);
		expect(pointer).not.toBeNull();
		const store = openTestMemoryStore(resolve(layout.dbDir, pointer as string));
		try {
			const attempt = store.db
				.prepare(
					"SELECT surface, retrieval_status, delivery_status, candidate_count, selected_count, source_session_id FROM retrieval_attempts WHERE surface = ?",
				)
				.get("file_context") as
				| {
						surface: string;
						retrieval_status: string;
						delivery_status: string;
						candidate_count: number;
						selected_count: number;
						source_session_id: string | null;
				  }
				| undefined;
			const event = store.db
				.prepare("SELECT source, event_type, payload_json FROM raw_events WHERE stream_id = ?")
				.get("session-file-context") as
				| { source: string; event_type: string; payload_json: string }
				| undefined;
			expect(attempt).toEqual({
				surface: "file_context",
				retrieval_status: "no_results",
				delivery_status: "not_attempted",
				candidate_count: 0,
				selected_count: 0,
				source_session_id: "session-file-context",
			});
			expect(event?.source).toBe("claude");
			expect(event?.event_type).toBe("tool_started");
			const stored = JSON.parse(event?.payload_json ?? "{}") as Record<string, unknown>;
			expect((stored._adapter as { schema_version?: string }).schema_version).toBe("1.0");
			expect((stored._normalized as { sourceHash?: string }).sourceHash).toMatch(/^[a-f0-9]{64}$/);
		} finally {
			store.close();
		}
	});

	it("P1-T041-04 preserves SessionStart timestamps through RPC and spool import", async () => {
		const root = await tempRoot();
		const dataDir = join(root, "data");
		process.env.CODEMEM_DATA_DIR = dataDir;
		const directAt = "2026-08-14T01:02:03.000Z";
		const spooledAt = "2026-08-14T02:03:04.000Z";
		let daemon = await startDaemon({ dataDir });
		await deliverHookEvent("claude", {
			hook_event_name: "SessionStart",
			session_id: "session-start-rpc",
			cwd: root,
			timestamp: directAt,
		});
		await daemon.stop();
		await deliverHookEvent(
			"claude",
			{
				hook_event_name: "SessionStart",
				session_id: "session-start-spool",
				cwd: root,
				timestamp: spooledAt,
			},
			{ rpcTimeoutMs: 25 },
		);
		daemon = await startDaemon({ dataDir });
		await daemon.stop();

		const layout = resolveStorageLayout(dataDir);
		const pointer = readCurrentDatabasePointer(layout);
		const store = openTestMemoryStore(resolve(layout.dbDir, pointer as string));
		try {
			const rows = store.db
				.prepare(
					"SELECT stream_id, started_at FROM raw_event_sessions WHERE stream_id IN (?, ?) ORDER BY stream_id",
				)
				.all("session-start-rpc", "session-start-spool") as Array<{
				stream_id: string;
				started_at: string | null;
			}>;
			expect(rows).toEqual([
				{ stream_id: "session-start-rpc", started_at: "2026-08-14T01:02:03.000000Z" },
				{ stream_id: "session-start-spool", started_at: "2026-08-14T02:03:04.000000Z" },
			]);
		} finally {
			store.close();
		}
	});
});
