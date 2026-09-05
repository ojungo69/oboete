#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readdirSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { pathToFileURL } from "node:url";

const repoRoot = resolve(import.meta.dirname, "..");
const packageRoot = join(repoRoot, "vendor/codemem");
const scriptPath = resolve(import.meta.filename);
const corePath = join(packageRoot, "packages/core/dist/index.js");
const hookRuntimePath = join(packageRoot, "packages/cli/dist/hook-runtime.js");
const safePath = `${dirname(process.execPath)}:/usr/local/bin:/usr/bin:/bin`;
const maxOutputBytes = 1024 * 1024;
const trackedChildren = new Set();
const trackedServers = new Set();

const agents = {
	claude: { command: "claude-hook-ingest" },
	codex: { command: "codex-hook-ingest" },
};

function commandFallback(command) {
	return command === "claude-hook-ingest" ? "" : '{"continue":true}';
}

function runtimeEnv(dataDir, root) {
	const home = join(root, "home");
	const temp = join(root, "tmp");
	mkdirSync(join(home, ".config"), { recursive: true, mode: 0o700 });
	mkdirSync(temp, { recursive: true, mode: 0o700 });
	return {
		PATH: safePath,
		HOME: home,
		XDG_CONFIG_HOME: join(home, ".config"),
		TMPDIR: temp,
		TERM: "dumb",
		NO_COLOR: "1",
		TZ: "UTC",
		AGENT_MEMORY_INTERNAL_RUN: "1",
		CODEMEM_DATA_DIR: dataDir,
		CODEMEM_EMBEDDING_DISABLED: "1",
		CODEMEM_PROJECT: "phase1-t056",
		CODEMEM_PLUGIN_LOG_PATH: join(root, "hook.log"),
	};
}

function hookPayload(workspace, sessionId, extra = {}) {
	return JSON.stringify({
		hook_event_name: "UserPromptSubmit",
		session_id: sessionId,
		cwd: workspace,
		timestamp: "2026-08-15T03:00:00.000Z",
		prompt: "Phase 1 T056 no-agent-blockage fixture",
		...extra,
	});
}

function appendOutput(current, chunk, child, label) {
	const next = current + String(chunk);
	if (Buffer.byteLength(next, "utf8") <= maxOutputBytes) return next;
	child.kill("SIGKILL");
	throw new Error(`${label} exceeded the output cap.`);
}

function runBuiltHook(agent, input, env, cwd, budget, options = {}) {
	return new Promise((resolveResult, rejectResult) => {
		let stdout = "";
		let stderr = "";
		let watchdogFired = false;
		const started = performance.now();
		const command = options.command ?? agents[agent].command;
		const child = spawn(process.execPath, [hookRuntimePath, command], {
			cwd,
			env,
			stdio: ["pipe", "pipe", "pipe"],
		});
		trackedChildren.add(child);
		child.stdout.on("data", (chunk) => {
			stdout = appendOutput(stdout, chunk, child, `${agent} stdout`);
		});
		child.stderr.on("data", (chunk) => {
			stderr = appendOutput(stderr, chunk, child, `${agent} stderr`);
		});
		child.stdin.on("error", () => {});
		child.once("error", rejectResult);
		const stdinTimer = options.stdinDelayMs
			? setTimeout(() => child.stdin.end(input), options.stdinDelayMs)
			: null;
		if (!stdinTimer) child.stdin.end(input);
		const watchdog = setTimeout(() => {
			watchdogFired = true;
			child.kill("SIGKILL");
		}, budget.outerWatchdogMs);
		child.once("close", (code, signal) => {
			clearTimeout(watchdog);
			if (stdinTimer) clearTimeout(stdinTimer);
			trackedChildren.delete(child);
			resolveResult({
				code,
				signal,
				stdout,
				stderr,
				watchdogFired,
				elapsedMs: performance.now() - started,
			});
		});
	});
}

function assertHookResult(label, agent, result, budget, command = agents[agent].command) {
	assert.equal(result.watchdogFired, false, `${label}/${agent} hit the external watchdog`);
	assert.equal(result.signal, null, `${label}/${agent} was killed by ${result.signal}`);
	assert.equal(result.code, 0, `${label}/${agent} exited ${result.code}: ${result.stderr.trim()}`);
	assert.equal(result.stdout, commandFallback(command), `${label}/${agent} fallback changed`);
	assert.ok(
		result.elapsedMs <= budget.clientHardCapMs,
		`${label}/${agent} took ${result.elapsedMs.toFixed(1)}ms (cap ${budget.clientHardCapMs}ms)`,
	);
}

function readyEntries(core, dataDir) {
	const readyDir = core.resolveSpoolLayout(dataDir).readyDir;
	if (!existsSync(readyDir)) return [];
	return readdirSync(readyDir)
		.filter((name) => name.endsWith(".json"))
		.sort()
		.map((name) => JSON.parse(readFileSync(join(readyDir, name), "utf8")));
}

function assertSpool(core, label, dataDir, expected, sessionId) {
	const entries = readyEntries(core, dataDir);
	if (expected === "none") {
		assert.deepEqual(entries, [], `${label} unexpectedly persisted a spool entry`);
		return;
	}
	assert.equal(entries.length, 1, `${label} expected one spool entry, got ${entries.length}`);
	const entry = entries[0];
	assert.equal(entry.method, "POST /v1/events");
	assert.equal(entry.quotaClass, expected);
	if (sessionId !== null) assert.equal(entry.body.event.nativeSessionId, sessionId);
}

function assertNoNeedles(value, needles, label) {
	const bytes = Buffer.isBuffer(value) ? value : Buffer.from(String(value));
	for (const needle of needles) {
		assert.equal(bytes.includes(Buffer.from(needle)), false, `${label} leaked private input`);
	}
}

function scanTree(root, needles) {
	if (!existsSync(root)) return;
	for (const entry of readdirSync(root, { withFileTypes: true })) {
		const path = join(root, entry.name);
		if (entry.isDirectory()) scanTree(path, needles);
		else if (entry.isFile()) assertNoNeedles(readFileSync(path), needles, path);
	}
}

async function fakeRpcServer(socketPath, mode, rpcMaxBytes) {
	mkdirSync(dirname(socketPath), { recursive: true, mode: 0o700 });
	const sockets = new Set();
	const state = { requests: [], offeredBytes: 0, responseBytes: 0, backpressure: false };
	const server = createServer((socket) => {
		sockets.add(socket);
		socket.on("error", () => {});
		socket.once("close", () => sockets.delete(socket));
		let input = Buffer.alloc(0);
		socket.on("data", (chunk) => {
			input = Buffer.concat([input, chunk]);
			const newline = input.indexOf(0x0a);
			if (newline < 0) return;
			const raw = input.subarray(0, newline).toString("utf8");
			state.requests.push(raw);
			const request = JSON.parse(raw);
			if (mode === "healthy") {
				socket.end(`${JSON.stringify({ id: request.id, result: { status: "committed" } })}\n`);
			} else if (mode === "large-pack") {
				const result =
					request.method === "POST /v1/context/pack"
						? {
								pack: {
									pack_text: "T056 large pack accepted",
									items: [{ metadata: "m".repeat(48 * 1024) }],
									metrics: { pack_tokens: 6 },
								},
							}
						: { status: "committed" };
				const response = `${JSON.stringify({ id: request.id, result })}\n`;
				state.responseBytes = Math.max(state.responseBytes, Buffer.byteLength(response));
				socket.end(response);
			} else if (mode === "socket-permission") {
				socket.end(
					`${JSON.stringify({ error: { code: "peer_denied", message: "denied", retryable: false } })}\n`,
				);
			} else if (mode === "protocol-mismatch") {
				socket.end(
					`${JSON.stringify({ error: { code: "protocol_mismatch", message: "incompatible", retryable: false } })}\n`,
				);
			} else if (mode === "partial-close") {
				socket.end(`{"id":"${request.id}`);
			} else if (mode === "partial-hold") {
				socket.write(`{"id":"${request.id}`);
			} else if (mode === "oversized-no-newline") {
				const response = Buffer.alloc(rpcMaxBytes * 16, 0x78);
				state.offeredBytes += response.length;
				state.backpressure ||= !socket.write(response);
			}
		});
	});
	trackedServers.add(server);
	await new Promise((resolveListen, rejectListen) => {
		server.once("error", rejectListen);
		server.listen(socketPath, resolveListen);
	});
	if (mode === "socket-permission" && process.getuid?.() !== 0) chmodSync(socketPath, 0o000);
	return {
		state,
		async close() {
			for (const socket of sockets) socket.destroy();
			await new Promise((resolveClose) => server.close(resolveClose));
			trackedServers.delete(server);
		},
	};
}

async function workerSpawnFailureGate(core, root) {
	const preloadPath = join(root, "worker-spawn-failure.mjs");
	writeFileSync(
		preloadPath,
		`import workerThreads from "node:worker_threads";
import { syncBuiltinESMExports } from "node:module";
if (workerThreads.isMainThread) {
	workerThreads.Worker = class {
		constructor() { throw new Error("injected worker spawn failure"); }
	};
	syncBuiltinESMExports();
}
`,
		{ mode: 0o600 },
	);
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "worker-spawn-failure", agent);
		const sessionId = `phase1-t056-worker-spawn-${agent}`;
		const result = await runBuiltHook(
			agent,
			hookPayload(item.workspace, sessionId),
			{ ...item.env, NODE_OPTIONS: `--import=${pathToFileURL(preloadPath).href}` },
			item.workspace,
			core.HOOK_DELIVERY_BUDGETS[agent],
		);
		assertHookResult(
			"worker-spawn-failure",
			agent,
			result,
			core.HOOK_DELIVERY_BUDGETS[agent],
		);
		assertSpool(core, `worker-spawn-failure/${agent}`, item.dataDir, "none", sessionId);
		console.log(`PASS worker-spawn-failure/${agent} ${result.elapsedMs.toFixed(1)}ms`);
	}
}

async function stdoutFlushStallGate(core, root) {
	const preloadPath = join(root, "stdout-flush-stall.mjs");
	writeFileSync(
		preloadPath,
		`import { isMainThread } from "node:worker_threads";
if (isMainThread) process.stdout.write = () => false;
`,
		{ mode: 0o600 },
	);
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "stdout-flush-stall", agent);
		const command = `${agent}-hook-inject`;
		const result = await runBuiltHook(
			agent,
			hookPayload(item.workspace, `phase1-t056-stdout-stall-${agent}`),
			{ ...item.env, NODE_OPTIONS: `--import=${pathToFileURL(preloadPath).href}` },
			item.workspace,
			core.HOOK_DELIVERY_BUDGETS[agent],
			{ command },
		);
		assert.equal(result.watchdogFired, false, `stdout-flush-stall/${agent} hit watchdog`);
		assert.equal(result.signal, null, `stdout-flush-stall/${agent} was killed`);
		assert.equal(result.code, 0, `stdout-flush-stall/${agent} exited ${result.code}`);
		assert.equal(result.stdout, "", `stdout-flush-stall/${agent} unexpectedly wrote output`);
		assert.ok(
			result.elapsedMs <= core.HOOK_DELIVERY_BUDGETS[agent].clientHardCapMs,
			`stdout-flush-stall/${agent} exceeded hard cap`,
		);
		console.log(`PASS stdout-flush-stall/${agent} ${result.elapsedMs.toFixed(1)}ms`);
	}
}

async function legitimateLargePackGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "large-pack-response", agent);
		const command = `${agent}-hook-inject`;
		const server = await fakeRpcServer(item.socketPath, "large-pack", core.RPC_MAX_BYTES);
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(item.workspace, `phase1-t056-large-pack-${agent}`),
				item.env,
				item.workspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
				{ command },
			);
			assert.equal(result.watchdogFired, false);
			assert.equal(result.signal, null);
			assert.equal(result.code, 0, result.stderr);
			assert.ok(result.elapsedMs <= core.HOOK_DELIVERY_BUDGETS[agent].clientHardCapMs);
			const output = JSON.parse(result.stdout);
			assert.equal(output.continue, true);
			assert.match(output.hookSpecificOutput?.additionalContext ?? "", /T056 large pack accepted/);
			assert.ok(server.state.responseBytes > core.RPC_MAX_BYTES);
			assert.ok(server.state.responseBytes < 256 * 1024);
			assertSpool(core, `large-pack-response/${agent}`, item.dataDir, "none", null);
			console.log(`PASS large-pack-response/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await server.close();
		}
	}
}

function fixture(core, root, label, agent) {
	const fixtureRoot = join(root, label, agent);
	const dataDir = join(fixtureRoot, "data");
	const workspace = join(fixtureRoot, "workspace");
	mkdirSync(join(workspace, ".git"), { recursive: true, mode: 0o700 });
	return {
		fixtureRoot,
		dataDir,
		workspace,
		env: runtimeEnv(dataDir, fixtureRoot),
		socketPath: core.resolveStorageLayout(dataDir).socketPath,
	};
}

async function faultPair(core, root, label, mode, expectedSpool) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, label, agent);
		const sessionId = `phase1-t056-${label}-${agent}`;
		const server = mode ? await fakeRpcServer(item.socketPath, mode, core.RPC_MAX_BYTES) : null;
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(item.workspace, sessionId),
				item.env,
				item.workspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assertHookResult(label, agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
			assertSpool(core, `${label}/${agent}`, item.dataDir, expectedSpool, sessionId);
			assert.ok((server?.state.requests.length ?? 0) <= 1, `${label}/${agent} retried RPC`);
			if (mode === "socket-permission" && process.getuid?.() !== 0) {
				assert.equal(server.state.requests.length, 0, `${label}/${agent} did not get EACCES`);
			}
			console.log(`PASS ${label}/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await server?.close();
		}
	}
}

async function injectFailOpenGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "hung-inject", agent);
		const sessionId = `phase1-t056-hung-inject-${agent}`;
		const command = `${agent}-hook-inject`;
		const server = await fakeRpcServer(item.socketPath, "hung", core.RPC_MAX_BYTES);
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(item.workspace, sessionId),
				item.env,
				item.workspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
				{ command },
			);
			assertHookResult("hung-inject", agent, result, core.HOOK_DELIVERY_BUDGETS[agent], command);
			assertSpool(core, `hung-inject/${agent}`, item.dataDir, "normal", sessionId);
			console.log(`PASS hung-inject/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await server.close();
		}
	}
}

async function oversizedInputGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "raw-over-256kib", agent);
		const sessionId = `phase1-t056-oversized-${agent}`;
		const input = hookPayload(item.workspace, sessionId, { prompt: "x".repeat(256 * 1024) });
		assert.ok(Buffer.byteLength(input, "utf8") > 256 * 1024);
		const result = await runBuiltHook(
			agent,
			input,
			item.env,
			item.workspace,
			core.HOOK_DELIVERY_BUDGETS[agent],
		);
		assertHookResult("raw-over-256kib", agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
		assertSpool(core, `raw-over-256kib/${agent}`, item.dataDir, "none", sessionId);
		console.log(`PASS raw-over-256kib/${agent} ${result.elapsedMs.toFixed(1)}ms`);
	}
}

async function backpressureGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "newline-free-over-32kib", agent);
		const sessionId = `phase1-t056-backpressure-${agent}`;
		const server = await fakeRpcServer(
			item.socketPath,
			"oversized-no-newline",
			core.RPC_MAX_BYTES,
		);
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(item.workspace, sessionId),
				item.env,
				item.workspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assertHookResult(
				"newline-free-over-32kib",
				agent,
				result,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assertSpool(core, `newline-free-over-32kib/${agent}`, item.dataDir, "normal", sessionId);
			assert.ok(server.state.offeredBytes > core.RPC_MAX_BYTES);
			assert.equal(server.state.backpressure, true, "fake peer did not exercise socket backpressure");
			console.log(`PASS newline-free-over-32kib/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await server.close();
		}
	}
}

function startLockHolder(dataDir) {
	let stderr = "";
	const child = spawn(process.execPath, [scriptPath, "__hold_spool_lock", dataDir], {
		cwd: packageRoot,
		stdio: ["ignore", "ignore", "pipe", "ipc"],
	});
	trackedChildren.add(child);
	child.stderr.on("data", (chunk) => {
		stderr = appendOutput(stderr, chunk, child, "lock holder stderr");
	});
	const ready = new Promise((resolveReady, rejectReady) => {
		const timeout = setTimeout(() => {
			child.kill("SIGKILL");
			rejectReady(new Error("spool lock holder did not become ready"));
		}, 5_000);
		child.once("message", (message) => {
			if (message?.type !== "ready") return;
			clearTimeout(timeout);
			resolveReady();
		});
		child.once("exit", (code, signal) => {
			clearTimeout(timeout);
			trackedChildren.delete(child);
			rejectReady(new Error(`spool lock holder exited ${code}/${signal}: ${stderr}`));
		});
	});
	return {
		child,
		ready,
		async close() {
			if (child.exitCode !== null || child.signalCode !== null) return;
			const exited = new Promise((resolveExit) => child.once("exit", resolveExit));
			child.send({ type: "release" });
			if ((await Promise.race([exited.then(() => true), sleep(2_000).then(() => false)])) === false) {
				child.kill("SIGKILL");
				await exited;
			}
		},
	};
}

async function longLockGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "long-spool-lock", agent);
		const sessionId = `phase1-t056-lock-${agent}`;
		const holder = startLockHolder(item.dataDir);
		await holder.ready;
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(item.workspace, sessionId),
				item.env,
				item.workspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assertHookResult("long-spool-lock", agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
			assertSpool(core, `long-spool-lock/${agent}`, item.dataDir, "none", sessionId);
			console.log(`PASS long-spool-lock/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await holder.close();
		}
	}
}

async function diskFullGate(core, root) {
	const preloadPath = join(root, "disk-full-hook-worker.mjs");
	writeFileSync(
		preloadPath,
		`import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { isMainThread } from "node:worker_threads";
if (!isMainThread) {
	const writeFileSync = fs.writeFileSync.bind(fs);
	fs.writeFileSync = (path, ...args) => {
		if (String(path).includes("/spool/tmp/") && String(path).endsWith(".json.tmp")) {
			const error = new Error("injected spool disk full");
			error.code = "ENOSPC";
			throw error;
		}
		return writeFileSync(path, ...args);
	};
	syncBuiltinESMExports();
}
`,
		{ mode: 0o600 },
	);
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "disk-full", agent);
		const sessionId = `phase1-t056-disk-full-${agent}`;
		const result = await runBuiltHook(
			agent,
			hookPayload(item.workspace, sessionId),
			{ ...item.env, NODE_OPTIONS: `--import=${pathToFileURL(preloadPath).href}` },
			item.workspace,
			core.HOOK_DELIVERY_BUDGETS[agent],
		);
		assertHookResult("disk-full", agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
		assertSpool(core, `disk-full/${agent}`, item.dataDir, "none", sessionId);
		assert.match(result.stderr, /\[codemem\] spool disk full; event was dropped\./);
		console.log(`PASS disk-full/${agent} ${result.elapsedMs.toFixed(1)}ms`);
	}
}

async function catastrophicRegexGate(core, root, field) {
	for (const agent of Object.keys(agents)) {
		const label = `catastrophic-${field.replace("_", "-")}`;
		const item = fixture(core, root, label, agent);
		writeFileSync(
			join(item.workspace, ".agent-memory.toml"),
			`${field} = ["(a+)+$", "T056_METADATA_PRIVATE_[A-Za-z_-]+"]\n`,
			{ mode: 0o600 },
		);
		const secret = `ghp_${"Z".repeat(36)}`;
		const privateText = `T056_PRIVATE_${field}_${agent}`;
		const metadataSecret = `T056_METADATA_PRIVATE_${field}_${agent}`;
		const secretWorkspace = join(item.workspace, metadataSecret);
		mkdirSync(secretWorkspace, { recursive: true, mode: 0o700 });
		const needles = [secret, privateText, metadataSecret, "<private>"];
		const prompt = `${"a".repeat(26)}! ${secret} <private>${privateText}</private>`;
		const sessionId = metadataSecret;
		const server = await fakeRpcServer(item.socketPath, "protocol-mismatch", core.RPC_MAX_BYTES);
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(secretWorkspace, sessionId, { prompt, tool_use_id: metadataSecret }),
				item.env,
				secretWorkspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assertHookResult(label, agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
			assertSpool(core, `${label}/${agent}`, item.dataDir, "normal", null);
			const entries = readyEntries(core, item.dataDir);
			assert.equal(entries[0].redaction.redaction_degraded, true);
			assertNoNeedles(`${result.stdout}\n${result.stderr}`, needles, `${label}/${agent} output`);
			assertNoNeedles(server.state.requests.join("\n"), needles, `${label}/${agent} RPC`);
			scanTree(item.fixtureRoot, needles);
			console.log(`PASS ${label}/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await server.close();
		}
	}
}

async function safeCustomRegexGate(core, root) {
	const hexadecimal = "0123456789abcdef";
	const cases = [
		{
			field: "secret_regex",
			pattern: "customer-[0-9]+",
			prompt: "customer-1234",
			sensitivity: "secret",
		},
		{
			field: "private_regex",
			pattern: "PRIVATE_SAFE_[0-9]+",
			prompt: "PRIVATE_SAFE_1234",
			sensitivity: "private",
		},
		{
			field: null,
			prompt: `shpat_${Array.from({ length: 32 }, (_, index) => hexadecimal[index % hexadecimal.length]).join("")}`,
			sensitivity: "secret",
		},
	];
	for (const itemCase of cases) {
		for (const agent of Object.keys(agents)) {
			const label = itemCase.field
				? `safe-${itemCase.field.replace("_", "-")}`
				: "pinned-gitleaks";
			const item = fixture(core, root, label, agent);
			if (itemCase.field) {
				writeFileSync(
					join(item.workspace, ".agent-memory.toml"),
					`${itemCase.field} = ["${itemCase.pattern}"]\n`,
					{ mode: 0o600 },
				);
			}
			const sessionId = `phase1-t056-${label}-${agent}`;
			const server = await fakeRpcServer(item.socketPath, "protocol-mismatch", core.RPC_MAX_BYTES);
			try {
				const result = await runBuiltHook(
					agent,
					hookPayload(item.workspace, sessionId, { prompt: itemCase.prompt }),
					item.env,
					item.workspace,
					core.HOOK_DELIVERY_BUDGETS[agent],
				);
				assertHookResult(label, agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
				assertSpool(core, `${label}/${agent}`, item.dataDir, "normal", sessionId);
				const entry = readyEntries(core, item.dataDir)[0];
				assert.equal(entry.redaction.redaction_degraded, false);
				assert.equal(entry.redaction.sensitivity, itemCase.sensitivity);
				assertNoNeedles(JSON.stringify(entry), [itemCase.prompt], `${label}/${agent} spool`);
				console.log(`PASS ${label}/${agent} ${result.elapsedMs.toFixed(1)}ms`);
			} finally {
				await server.close();
			}
		}
	}
}

async function timeoutEdgeSessionEndGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "timeout-edge-session-end", agent);
		const sessionId = `phase1-t056-session-end-${agent}`;
		const privateReason = `T056_SESSION_END_PRIVATE_${agent}`;
		const server = await fakeRpcServer(item.socketPath, "hung", core.RPC_MAX_BYTES);
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(item.workspace, sessionId, {
					hook_event_name: "SessionEnd",
					prompt: undefined,
					reason: `<private>${privateReason}</private>`,
				}),
				item.env,
				item.workspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assertHookResult(
				"timeout-edge-session-end",
				agent,
				result,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assert.ok(
				result.elapsedMs >= core.HOOK_DELIVERY_BUDGETS[agent].rpcCutoffMs - 250,
				`SessionEnd/${agent} returned before the RPC cutoff: ${result.elapsedMs.toFixed(1)}ms`,
			);
			assertSpool(
				core,
				`timeout-edge-session-end/${agent}`,
				item.dataDir,
				"reserved",
				sessionId,
			);
			assertNoNeedles(
				JSON.stringify(readyEntries(core, item.dataDir)[0]),
				[privateReason],
				`SessionEnd/${agent} spool`,
			);
			console.log(`PASS timeout-edge-session-end/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await server.close();
		}
	}
}

async function slowInputSessionEndGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "slow-input-session-end", agent);
		const sessionId = `phase1-t056-slow-input-${agent}`;
		const budget = core.HOOK_DELIVERY_BUDGETS[agent];
		const stdinDelayMs = budget.clientHardCapMs - budget.spoolReserveMs - 350;
		const server = await fakeRpcServer(item.socketPath, "hung", core.RPC_MAX_BYTES);
		try {
			const result = await runBuiltHook(
				agent,
				hookPayload(item.workspace, sessionId, {
					hook_event_name: "SessionEnd",
					prompt: undefined,
					reason: "slow input",
				}),
				item.env,
				item.workspace,
				budget,
				{ stdinDelayMs },
			);
			assertHookResult("slow-input-session-end", agent, result, budget);
			assert.ok(result.elapsedMs >= stdinDelayMs, `${agent} returned before stdin completed`);
			assertSpool(core, `slow-input-session-end/${agent}`, item.dataDir, "reserved", sessionId);
			console.log(`PASS slow-input-session-end/${agent} ${result.elapsedMs.toFixed(1)}ms`);
		} finally {
			await server.close();
		}
	}
}

async function timestampLessSessionEndReplayGate(core, root) {
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "session-end-replay", agent);
		const sessionId = `phase1-t056-session-end-replay-${agent}`;
		const input = JSON.stringify({
			hook_event_name: "SessionEnd",
			session_id: sessionId,
			cwd: item.workspace,
			reason: "other",
		});
		for (let attempt = 0; attempt < 20; attempt += 1) {
			const result = await runBuiltHook(
				agent,
				input,
				item.env,
				item.workspace,
				core.HOOK_DELIVERY_BUDGETS[agent],
			);
			assertHookResult("session-end-replay", agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
		}
		assertSpool(core, `session-end-replay/${agent}`, item.dataDir, "reserved", sessionId);
		console.log(`PASS timestamp-less SessionEnd replay/${agent} x20 exactly-one spool entry`);
	}
}

function redactionWorkerStallPreload(root, name, spoolDelayMs = 0) {
	const preloadPath = join(root, name);
	writeFileSync(
		preloadPath,
		`import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { isMainThread, workerData } from "node:worker_threads";
if (!isMainThread && workerData?.role === "redaction-worker") {
	Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 60_000);
}
if (!isMainThread && workerData?.role === "hook-runtime" && ${spoolDelayMs} > 0) {
	const writeFileSync = fs.writeFileSync.bind(fs);
	fs.writeFileSync = (path, ...args) => {
		if (String(path).includes("/spool/tmp/") && String(path).endsWith(".json.tmp")) {
			fs.appendFileSync(process.env.CODEMEM_TEST_SPOOL_DELAY_LOG, "hit\\n", { mode: 0o600 });
			Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ${spoolDelayMs});
		}
		return writeFileSync(path, ...args);
	};
	syncBuiltinESMExports();
}
`,
		{ mode: 0o600 },
	);
	return preloadPath;
}

async function redactionWorkerStallGate(core, root) {
	const preloadPath = redactionWorkerStallPreload(root, "stall-redaction-worker.mjs");
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "redaction-worker-stall", agent);
		const sessionId = `phase1-t056-redaction-stall-${agent}`;
		const result = await runBuiltHook(
			agent,
			hookPayload(item.workspace, sessionId, {
				hook_event_name: "SessionEnd",
				prompt: undefined,
				reason: "redaction worker stalled",
			}),
			{ ...item.env, NODE_OPTIONS: `--import=${pathToFileURL(preloadPath).href}` },
			item.workspace,
			core.HOOK_DELIVERY_BUDGETS[agent],
		);
		assertHookResult("redaction-worker-stall", agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
		assertSpool(core, `redaction-worker-stall/${agent}`, item.dataDir, "reserved", null);
		assert.equal(readyEntries(core, item.dataDir)[0].redaction.redaction_degraded, true);
		console.log(`PASS redaction-worker-stall/${agent} ${result.elapsedMs.toFixed(1)}ms`);
	}
}

async function redactionWorkerSlowInputGate(core, root) {
	const preloadPath = redactionWorkerStallPreload(
		root,
		"stall-redaction-worker-slow-input.mjs",
		400,
	);
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "redaction-worker-slow-input", agent);
		const spoolDelayLog = join(item.fixtureRoot, "spool-delay.log");
		const sessionId = `phase1-t056-redaction-slow-input-${agent}`;
		const budget = core.HOOK_DELIVERY_BUDGETS[agent];
		const stdinDelayMs = budget.clientHardCapMs - budget.spoolReserveMs - 100;
		const result = await runBuiltHook(
			agent,
			hookPayload(item.workspace, sessionId, {
				hook_event_name: "SessionEnd",
				prompt: undefined,
				reason: "redaction worker stalled after slow input",
			}),
			{
				...item.env,
				CODEMEM_TEST_SPOOL_DELAY_LOG: spoolDelayLog,
				NODE_OPTIONS: `--import=${pathToFileURL(preloadPath).href}`,
			},
			item.workspace,
			budget,
			{ stdinDelayMs },
		);
		assertHookResult("redaction-worker-slow-input", agent, result, budget);
		assertSpool(core, `redaction-worker-slow-input/${agent}`, item.dataDir, "reserved", null);
		assert.equal(readFileSync(spoolDelayLog, "utf8"), "hit\n");
		assert.equal(readyEntries(core, item.dataDir)[0].redaction.redaction_degraded, true);
		console.log(`PASS redaction-worker-slow-input/${agent} ${result.elapsedMs.toFixed(1)}ms`);
	}
}

async function wholeRuntimeStallGate(core, root) {
	const preloadPath = join(root, "stall-hook-worker.mjs");
	writeFileSync(
		preloadPath,
		'import { isMainThread } from "node:worker_threads";\nif (!isMainThread) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 60_000);\n',
		{ mode: 0o600 },
	);
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "whole-runtime-stall", agent);
		const sessionId = `phase1-t056-runtime-stall-${agent}`;
		const result = await runBuiltHook(
			agent,
			hookPayload(item.workspace, sessionId),
			{ ...item.env, NODE_OPTIONS: `--import=${pathToFileURL(preloadPath).href}` },
			item.workspace,
			core.HOOK_DELIVERY_BUDGETS[agent],
		);
		assertHookResult("whole-runtime-stall", agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
		assert.ok(
			result.elapsedMs >= core.HOOK_DELIVERY_BUDGETS[agent].clientHardCapMs - 250,
			`whole-runtime-stall/${agent} did not reach the supervisor cap`,
		);
		assertSpool(core, `whole-runtime-stall/${agent}`, item.dataDir, "none", sessionId);
		console.log(`PASS whole-runtime-stall/${agent} ${result.elapsedMs.toFixed(1)}ms`);
	}
}

function percentile95(values) {
	const ordered = [...values].sort((left, right) => left - right);
	return ordered[Math.ceil(ordered.length * 0.95) - 1];
}

async function healthyP95Gate(core, root, targetMs) {
	const metrics = [];
	for (const agent of Object.keys(agents)) {
		const item = fixture(core, root, "healthy-p95", agent);
		const server = await fakeRpcServer(item.socketPath, "healthy", core.RPC_MAX_BYTES);
		const samples = [];
		try {
			for (let index = 0; index < 22; index += 1) {
				const sessionId = `phase1-t056-healthy-${agent}-${index}`;
				const result = await runBuiltHook(
					agent,
					hookPayload(item.workspace, sessionId),
					item.env,
					item.workspace,
					core.HOOK_DELIVERY_BUDGETS[agent],
				);
				assertHookResult("healthy-p95", agent, result, core.HOOK_DELIVERY_BUDGETS[agent]);
				if (index >= 2) samples.push(result.elapsedMs);
			}
			assertSpool(core, `healthy-p95/${agent}`, item.dataDir, "none", "unused");
			assert.equal(server.state.requests.length, 22);
			const p95 = percentile95(samples);
			assert.ok(Number.isFinite(p95));
			metrics.push({ agent, p95, targetMs, met: p95 < targetMs });
			console.log(
				`METRIC healthy/${agent} p95=${p95.toFixed(1)}ms target=${targetMs}ms ${p95 < targetMs ? "met" : "miss"}`,
			);
		} finally {
			await server.close();
		}
	}
	return metrics;
}

async function globalScannerDeadlineGate(core, root) {
	const fixtureRoot = join(root, "global-scanner-deadline");
	const dataDir = join(fixtureRoot, "data");
	const configPath = join(fixtureRoot, "codemem.json");
	const privateText = "T056_GLOBAL_SCANNER_PRIVATE";
	mkdirSync(fixtureRoot, { recursive: true, mode: 0o700 });
	writeFileSync(
		configPath,
		JSON.stringify({
			secret_scanner: { rules: [{ kind: "catastrophic", pattern: "(a+)+$" }] },
		}),
		{ mode: 0o600 },
	);
	const previousConfig = process.env.CODEMEM_CONFIG;
	const request = {
		adapter_version: "phase1-t056",
		native_cli_version: "phase1-t056",
		normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
		local_api_version: core.LOCAL_API_VERSION,
		capability_hash: core.RPC_CAPABILITY_HASH,
	};
	let handle;
	try {
		delete process.env.CODEMEM_CONFIG;
		handle = await core.startDaemon({ dataDir });
		const seeded = await core.callDaemonRpc(handle.socketPath, {
			...request,
			id: "phase1-t056-global-scanner-seed",
			method: "POST /v1/memories/record",
			body: {
				idempotencyKey: "phase1-t056-global-scanner-seed",
				kind: "discovery",
				title: "maintenance scanner seed",
				body: `${"a".repeat(26)}!`,
			},
		});
		assert.ok("result" in seeded, `maintenance seed failed: ${JSON.stringify(seeded)}`);
		await handle.stop();

		process.env.CODEMEM_CONFIG = configPath;
		handle = await core.startDaemon({ dataDir });
		let ready = false;
		for (let attempt = 0; attempt < 100; attempt += 1) {
			const health = await core.callDaemonRpc(handle.socketPath, {
				...request,
				id: `phase1-t056-global-scanner-ready-${attempt}`,
				method: "GET /v1/health",
				body: {},
			});
			ready = "result" in health && health.result.maintenanceMode === false;
			if (ready) break;
			await sleep(10);
		}
		assert.equal(ready, true, "daemon did not leave startup maintenance");
		const started = performance.now();
		const recorded = await core.callDaemonRpc(
			handle.socketPath,
			{
				...request,
				id: "phase1-t056-global-scanner-record",
				method: "POST /v1/memories/record",
				body: {
					idempotencyKey: "phase1-t056-global-scanner-record",
					kind: "discovery",
					title: "global scanner deadline",
					body: `${"a".repeat(26)}! ${privateText}`,
				},
			},
			{ timeoutMs: 1_000 },
		);
		const elapsedMs = performance.now() - started;
		assert.ok("result" in recorded, `global scanner record failed: ${JSON.stringify(recorded)}`);
		assert.ok(elapsedMs < 1_000, `global scanner blocked daemon for ${elapsedMs.toFixed(1)}ms`);

		const submitted = await core.callDaemonRpc(handle.socketPath, {
			...request,
			id: "phase1-t056-global-scanner-job",
			method: "POST /v1/jobs",
			body: { kind: "secrets.scan", args: { limit: 1 }, dryRun: true },
		});
		assert.ok("result" in submitted, `maintenance job submit failed: ${JSON.stringify(submitted)}`);
		const jobId = String(submitted.result.jobId);
		let job;
		for (let attempt = 0; attempt < 100; attempt += 1) {
			const response = await core.callDaemonRpc(handle.socketPath, {
				...request,
				id: `phase1-t056-global-scanner-job-${attempt}`,
				method: "GET /v1/jobs/:id",
				body: { id: jobId },
			});
			if ("result" in response) {
				job = response.result.job;
				if (job?.state === "failed") break;
			}
			await sleep(10);
		}
		assert.equal(job?.state, "failed");
		assert.equal(job?.error?.code, "redaction_degraded");
		const health = await core.callDaemonRpc(handle.socketPath, {
			...request,
			id: "phase1-t056-global-scanner-health",
			method: "GET /v1/health",
			body: {},
		});
		assert.ok("result" in health, "daemon stopped responding after global scanner timeout");
		const doctor = await core.callDaemonRpc(handle.socketPath, {
			...request,
			id: "phase1-t056-global-scanner-doctor",
			method: "GET /v1/doctor",
			body: {},
		});
		assert.ok("result" in doctor);
		assert.deepEqual(doctor.result.diagnostics.redaction, {
			status: "warning",
			degradedDeliveries: 2,
			workerDeadlineMs: core.REDACTION_WORKER_DEADLINE_MS,
		});
		console.log(
			`PASS global write/maintenance scanner deadline + daemon continuity ${elapsedMs.toFixed(1)}ms`,
		);
	} finally {
		await handle?.stop();
		if (previousConfig === undefined) delete process.env.CODEMEM_CONFIG;
		else process.env.CODEMEM_CONFIG = previousConfig;
	}
	scanTree(fixtureRoot, [privateText]);
}

async function doctorGate(core, root) {
	const fixtureRoot = join(root, "doctor");
	const dataDir = join(fixtureRoot, "data");
	const workspace = join(fixtureRoot, "workspace");
	mkdirSync(join(workspace, ".git"), { recursive: true, mode: 0o700 });
	const preloadPath = redactionWorkerStallPreload(root, "doctor-stall-redaction-worker.mjs");
	const privateText = "T056_DOCTOR_PRIVATE";
	const env = runtimeEnv(dataDir, fixtureRoot);
	let handle = await core.startDaemon({ dataDir });
	const doctor = () =>
		core.callDaemonRpc(handle.socketPath, {
			id: `phase1-t056-doctor-${randomUUID()}`,
			method: "GET /v1/doctor",
			adapter_version: "phase1-t056",
			native_cli_version: "phase1-t056",
			normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
			local_api_version: core.LOCAL_API_VERSION,
			capability_hash: core.RPC_CAPABILITY_HASH,
			body: {},
		});
	let p95TargetMs;
	try {
		const response = await doctor();
		assert.ok("result" in response, `doctor failed: ${JSON.stringify(response)}`);
		assert.deepEqual(response.result.diagnostics.hookDelivery, {
			implementation: "node-fallback",
			p95TargetMs: 150,
			budgets: core.HOOK_DELIVERY_BUDGETS,
		});
		assert.deepEqual(response.result.diagnostics.redaction, {
			status: "ok",
			degradedDeliveries: 0,
			workerDeadlineMs: core.REDACTION_WORKER_DEADLINE_MS,
		});
		p95TargetMs = response.result.diagnostics.hookDelivery.p95TargetMs;

		const safeRecord = await core.callDaemonRpc(handle.socketPath, {
			id: "phase1-t056-safe-record",
			method: "POST /v1/memories/record",
			adapter_version: "phase1-t056",
			native_cli_version: "phase1-t056",
			normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
			local_api_version: core.LOCAL_API_VERSION,
			capability_hash: core.RPC_CAPABILITY_HASH,
			body: {
				idempotencyKey: "phase1-t056-safe-record",
				kind: "discovery",
				title: "safe worker positive control",
				body: "ordinary text",
			},
		});
		assert.ok("result" in safeRecord, `safe daemon intake failed: ${JSON.stringify(safeRecord)}`);
		const afterSafeRecord = await doctor();
		assert.ok("result" in afterSafeRecord);
		assert.equal(afterSafeRecord.result.diagnostics.redaction.degradedDeliveries, 0);

		const hookResult = await runBuiltHook(
			"codex",
			hookPayload(workspace, "phase1-t056-doctor-degraded", {
				prompt: `<private>${privateText}</private>`,
			}),
			{ ...env, NODE_OPTIONS: `--import=${pathToFileURL(preloadPath).href}` },
			workspace,
			core.HOOK_DELIVERY_BUDGETS.codex,
		);
		assertHookResult("doctor-degraded-delivery", "codex", hookResult, core.HOOK_DELIVERY_BUDGETS.codex);
		assertSpool(core, "doctor-degraded-delivery/codex", dataDir, "none", "unused");
		assertNoNeedles(`${hookResult.stdout}\n${hookResult.stderr}`, [privateText], "doctor hook output");

		const degraded = await doctor();
		assert.ok("result" in degraded, `doctor failed: ${JSON.stringify(degraded)}`);
		assert.deepEqual(degraded.result.diagnostics.redaction, {
			status: "warning",
			degradedDeliveries: 1,
			workerDeadlineMs: core.REDACTION_WORKER_DEADLINE_MS,
		});

		await handle.stop();
		handle = await core.startDaemon({ dataDir });
		const restarted = await doctor();
		assert.ok("result" in restarted, `doctor after restart failed: ${JSON.stringify(restarted)}`);
		assert.deepEqual(restarted.result.diagnostics.redaction, {
			status: "warning",
			degradedDeliveries: 1,
			workerDeadlineMs: core.REDACTION_WORKER_DEADLINE_MS,
		});
		console.log(
			"PASS GET /v1/doctor safe intake + persistent warning/degradedDeliveries=1 after restart",
		);
	} finally {
		await handle.stop();
	}
	scanTree(fixtureRoot, [privateText]);
	return p95TargetMs;
}

async function holdSpoolLock() {
	const dataDir = process.argv[3];
	assert.ok(dataDir, "lock holder data dir missing");
	const core = await import(pathToFileURL(corePath).href);
	const lock = core.acquireSpoolLock(dataDir, 250);
	try {
		process.send?.({ type: "ready" });
		await new Promise((resolveRelease) => process.once("message", resolveRelease));
	} finally {
		lock.close();
	}
}

async function main() {
	assert.equal(process.platform, "linux", "T056 Unix-socket gate requires Linux");
	assert.ok(existsSync(corePath), "build packages/core before running the T056 harness");
	assert.ok(existsSync(hookRuntimePath), "build packages/cli before running the T056 harness");
	const core = await import(pathToFileURL(corePath).href);
	assert.deepEqual(core.HOOK_DELIVERY_BUDGETS, {
		claude: {
			clientHardCapMs: 2_000,
			rpcCutoffMs: 1_500,
			spoolReserveMs: 500,
			spoolLockWaitMs: 100,
			fsyncMarginMs: 400,
			outerWatchdogMs: 3_000,
		},
		codex: {
			clientHardCapMs: 1_500,
			rpcCutoffMs: 1_000,
			spoolReserveMs: 500,
			spoolLockWaitMs: 100,
			fsyncMarginMs: 400,
			outerWatchdogMs: 5_000,
		},
	});

	const root = mkdtempSync(join(tmpdir(), "codemem-phase1-t056-"));
	const globalWatchdog = setTimeout(() => {
		for (const child of trackedChildren) child.kill("SIGKILL");
		process.stderr.write("T056 global safety watchdog expired.\n");
		process.exit(124);
	}, 120_000);
	try {
		await faultPair(core, root, "daemon-absent", null, "normal");
		await workerSpawnFailureGate(core, root);
		await faultPair(core, root, "socket-peer-denied", "socket-permission", "normal");
		await faultPair(core, root, "protocol-mismatch", "protocol-mismatch", "normal");
		await diskFullGate(core, root);
		await oversizedInputGate(core, root);
		await faultPair(core, root, "hung-daemon", "hung", "normal");
		await injectFailOpenGate(core, root);
		await faultPair(core, root, "partial-close", "partial-close", "normal");
		await faultPair(core, root, "partial-hold", "partial-hold", "normal");
		await backpressureGate(core, root);
		await legitimateLargePackGate(core, root);
		await longLockGate(core, root);
		await safeCustomRegexGate(core, root);
		await catastrophicRegexGate(core, root, "private_regex");
		await catastrophicRegexGate(core, root, "secret_regex");
		await timeoutEdgeSessionEndGate(core, root);
		await slowInputSessionEndGate(core, root);
		await timestampLessSessionEndReplayGate(core, root);
		await redactionWorkerStallGate(core, root);
		await redactionWorkerSlowInputGate(core, root);
		await wholeRuntimeStallGate(core, root);
		await stdoutFlushStallGate(core, root);
		await globalScannerDeadlineGate(core, root);
		const p95TargetMs = await doctorGate(core, root);
		const p95Metrics = await healthyP95Gate(core, root, p95TargetMs);
		const p95Misses = p95Metrics.filter((metric) => !metric.met);
		if (p95Misses.length > 0) {
			console.warn(
				`WARN T056 p95 target miss (non-blocking per spec performance policy): ${p95Misses.map((metric) => `${metric.agent}=${metric.p95.toFixed(1)}ms>${metric.targetMs}ms`).join(", ")}`,
			);
		}
		console.log(
			`RESULT T056 fail-open=pass p95=${p95Misses.length === 0 ? "met" : "target-miss"}`,
		);
	} finally {
		clearTimeout(globalWatchdog);
		for (const child of trackedChildren) child.kill("SIGKILL");
		for (const server of trackedServers) server.close();
		rmSync(root, { recursive: true, force: true });
	}
}

if (process.argv[2] === "__hold_spool_lock") await holdSpoolLock();
else await main();
