#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
	chmodSync,
	closeSync,
	existsSync,
	fstatSync,
	mkdirSync,
	mkdtempSync,
	openSync,
	readFileSync,
	readdirSync,
	realpathSync,
	renameSync,
	rmSync,
	statSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { createConnection, createServer } from "node:net";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { pathToFileURL } from "node:url";

const repoRoot = resolve(import.meta.dirname, "..");
const packageRoot = join(repoRoot, "vendor/codemem");
const scriptPath = resolve(import.meta.filename);
const corePath = join(packageRoot, "packages/core/dist/index.js");
const cliPath = join(packageRoot, "packages/cli/dist/index.js");
const hookRuntimePath = join(packageRoot, "packages/cli/dist/hook-runtime.js");
const mcpPath = join(packageRoot, "packages/mcp-server/dist/stdio.js");
const safePath = `${dirname(process.execPath)}:/usr/local/bin:/usr/bin:/bin`;
const maxOutputBytes = 1024 * 1024;
const trackedChildren = new Set();
const require = createRequire(join(packageRoot, "packages/core/package.json"));
const Database = require("better-sqlite3");

async function waitUntil(label, predicate, timeoutMs = 15_000) {
	const deadline = Date.now() + timeoutMs;
	let lastError;
	while (Date.now() < deadline) {
		try {
			const value = await predicate();
			if (value) return value;
		} catch (error) {
			lastError = error;
		}
		await sleep(10);
	}
	throw new Error(`Timed out waiting for ${label}.`, { cause: lastError });
}

function runtimeEnv(dataDir, root, extra = {}) {
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
		CODEMEM_PROJECT: "phase1-t055",
		...extra,
	};
}

function appendOutput(current, chunk, child, label) {
	const next = current + String(chunk);
	if (Buffer.byteLength(next, "utf8") <= maxOutputBytes) return next;
	child.kill("SIGKILL");
	throw new Error(`${label} exceeded the output cap.`);
}

function startDaemonWorker(dataDir, env, options = {}) {
	let stdout = "";
	let stderr = "";
	let settled = false;
	const child = spawn(process.execPath, [scriptPath, "__daemon", dataDir], {
		cwd: packageRoot,
		env: { ...env, T055_WAIT_FOR_START: options.waitForStart ? "1" : "" },
		stdio: ["ignore", "pipe", "pipe", "ipc"],
	});
	trackedChildren.add(child);
	child.stdout.on("data", (chunk) => {
		stdout = appendOutput(stdout, chunk, child, "daemon stdout");
	});
	child.stderr.on("data", (chunk) => {
		stderr = appendOutput(stderr, chunk, child, "daemon stderr");
	});
	const ready = new Promise((resolveReady, rejectReady) => {
		const timeout = setTimeout(() => {
			if (settled) return;
			settled = true;
			child.kill("SIGKILL");
			rejectReady(new Error("daemon did not become ready within 20 seconds"));
		}, 20_000);
		const resolveOnce = (value) => {
			if (settled) return;
			settled = true;
			clearTimeout(timeout);
			resolveReady(value);
		};
		const rejectOnce = (error) => {
			if (settled) return;
			settled = true;
			clearTimeout(timeout);
			rejectReady(error);
		};
		child.on("message", (message) => {
			if (message?.type === "ready") {
				resolveOnce(message);
			} else if (message?.type === "error") {
				rejectOnce(new Error(message.message));
			}
		});
		child.once("error", (error) => {
			rejectOnce(error);
		});
		child.once("exit", (code, signal) => {
			trackedChildren.delete(child);
			if (!settled) {
				rejectOnce(
					new Error(
						`daemon exited before ready (code=${code}, signal=${signal}): ${stderr.trim()}`,
					),
				);
			}
		});
	});
	return {
		child,
		ready,
		start() {
			child.send({ type: "start" });
		},
		get stderr() {
			return stderr;
		},
		get stdout() {
			return stdout;
		},
	};
}

async function stopWorker(worker) {
	const child = worker.child;
	if (child.exitCode !== null || child.signalCode !== null) return;
	try {
		process.kill(child.pid, "SIGCONT");
	} catch {
		// It may not be stopped.
	}
	const exited = new Promise((resolveExit) => child.once("exit", resolveExit));
	child.send({ type: "stop" });
	const outcome = await Promise.race([exited.then(() => "exited"), sleep(5_000).then(() => "timeout")]);
	if (outcome === "timeout") child.kill("SIGKILL");
	await exited;
}

async function killWorker(worker) {
	if (worker.child.exitCode !== null || worker.child.signalCode !== null) return;
	const exited = new Promise((resolveExit) => worker.child.once("exit", resolveExit));
	worker.child.kill("SIGKILL");
	await exited;
}

async function pauseWorker(worker) {
	process.kill(worker.child.pid, "SIGSTOP");
	await waitUntil("daemon SIGSTOP", () => {
		const status = readFileSync(`/proc/${worker.child.pid}/status`, "utf8");
		return /^State:\s+T/m.test(status);
	});
}

function startCaptured(name, args, input, env, cwd) {
	return new Promise((resolveResult, rejectResult) => {
		let stdout = "";
		let stderr = "";
		let timedOut = false;
		const child = spawn(process.execPath, args, {
			cwd,
			env,
			stdio: ["pipe", "pipe", "pipe"],
		});
		trackedChildren.add(child);
		child.stdout.on("data", (chunk) => {
			stdout = appendOutput(stdout, chunk, child, `${name} stdout`);
		});
		child.stderr.on("data", (chunk) => {
			stderr = appendOutput(stderr, chunk, child, `${name} stderr`);
		});
		child.once("error", rejectResult);
		child.stdin.end(input);
		const timeout = setTimeout(() => {
			timedOut = true;
			child.kill("SIGKILL");
		}, 10_000);
		child.once("exit", (code, signal) => {
			clearTimeout(timeout);
			trackedChildren.delete(child);
			resolveResult({ name, code, signal, stdout, stderr, timedOut });
		});
	});
}

function rpcRequest(core, method, body, id = randomUUID()) {
	return {
		id,
		method,
		adapter_version: "phase1-t055",
		native_cli_version: "phase1-t055",
		normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
		local_api_version: core.LOCAL_API_VERSION,
		capability_hash: core.RPC_CAPABILITY_HASH,
		body,
	};
}

async function rpc(core, socketPath, method, body, id) {
	const response = await core.callDaemonRpc(socketPath, rpcRequest(core, method, body, id), {
		timeoutMs: 10_000,
	});
	assert.ok(!("error" in response), `${method} failed: ${JSON.stringify(response)}`);
	return response.result;
}

function currentDatabasePath(core, dataDir) {
	const layout = core.resolveStorageLayout(dataDir);
	const pointer = core.readCurrentDatabasePointer(layout);
	assert.ok(pointer, `canonical pointer missing: ${dataDir}`);
	return join(layout.dbDir, pointer);
}

function normalizedEvent(key, eventId = `event-${key}`) {
	return {
		schemaVersion: 1,
		eventId,
		idempotencyKey: key,
		agent: "codex",
		nativeSessionId: "phase1-t055-session",
		projectKey: "phase1-t055",
		workspaceKey: "phase1-t055-workspace",
		cwd: "/tmp/phase1-t055",
		kind: "user_prompted",
		occurredAt: "2026-08-15T00:00:00.000Z",
		payload: { text: "phase1 fault injection" },
		sourceHash: "a".repeat(64),
		sensitivity: "normal",
	};
}

function assertNoNeedles(value, needles, label) {
	const buffer = Buffer.isBuffer(value) ? value : Buffer.from(String(value));
	for (const needle of needles) {
		assert.equal(buffer.includes(Buffer.from(needle)), false, `${label} leaked ${needle}`);
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

async function surfacePrivacyGate(core, root) {
	const fixtureRoot = join(root, "surfaces");
	const dataDir = join(fixtureRoot, "data");
	const workspace = join(fixtureRoot, "workspace");
	const pluginLog = join(fixtureRoot, "plugin.log");
	mkdirSync(workspace, { recursive: true, mode: 0o700 });
	const env = runtimeEnv(dataDir, fixtureRoot, { CODEMEM_PLUGIN_LOG_PATH: pluginLog });
	const secret = `ghp_${"A".repeat(36)}`;
	const privateText = "T055_PRIVATE_NEVER_PERSIST_7f3a";
	const prompt = `visible ${secret} <private>${privateText}</private> visible-end`;
	const killedDaemon = startDaemonWorker(dataDir, env);
	const killedReady = await killedDaemon.ready;
	await killWorker(killedDaemon);
	assert.equal(existsSync(killedReady.socketPath), true, "killed daemon did not leave a stale socket");
	const hookPayload = (sessionId, timestamp) =>
		JSON.stringify({
			hook_event_name: "UserPromptSubmit",
			session_id: sessionId,
			prompt,
			cwd: workspace,
			timestamp,
		});
	const mcpInput = [
		{
			jsonrpc: "2.0",
			id: 1,
			method: "initialize",
			params: {
				protocolVersion: "2025-11-25",
				capabilities: {},
				clientInfo: { name: "phase1-t055", version: "1" },
			},
		},
		{ jsonrpc: "2.0", method: "notifications/initialized", params: {} },
		{
			jsonrpc: "2.0",
			id: 2,
			method: "tools/call",
			params: {
				name: "memory_remember",
				arguments: {
					kind: "decision",
					title: "T055 privacy fixture",
					body: prompt,
					confidence: 0.5,
					project: "phase1-t055",
				},
			},
		},
	]
		.map((message) => JSON.stringify(message))
		.join("\n");
	const cliInput = JSON.stringify({
		session_stream_id: "phase1-t055-cli",
		event_type: "prompt",
		event_id: "phase1-t055-cli-event-1",
		cwd: workspace,
		project: "phase1-t055",
		ts_wall_ms: 1786668903000,
		payload: {
			_adapter: {
				event_type: "prompt",
				ts: "2026-08-15T00:55:03.000Z",
				payload: { text: prompt },
			},
		},
	});
	const results = await Promise.all([
		startCaptured(
			"hook-ingest",
			[hookRuntimePath, "codex-hook-ingest"],
			hookPayload("phase1-t055-hook-ingest", "2026-08-15T00:55:01.000Z"),
			env,
			workspace,
		),
		startCaptured(
			"hook-inject",
			[hookRuntimePath, "codex-hook-inject"],
			hookPayload("phase1-t055-hook-inject", "2026-08-15T00:55:02.000Z"),
			env,
			workspace,
		),
		startCaptured("mcp", [mcpPath], `${mcpInput}\n`, env, workspace),
		startCaptured("cli", [cliPath, "enqueue-raw-event"], cliInput, env, workspace),
	]);
	for (const result of results) {
		assert.equal(result.timedOut, false, `${result.name} timed out`);
		assert.equal(result.code, 0, `${result.name} failed: ${result.stderr.trim()}`);
		assert.equal(result.signal, null, `${result.name} was killed`);
		assertNoNeedles(`${result.stdout}\n${result.stderr}`, [secret, privateText], `${result.name} output`);
	}
	for (const result of results.filter((item) => item.name.startsWith("hook-"))) {
		assert.deepEqual(JSON.parse(result.stdout.trim()), { continue: true });
	}
	const mcpMessages = results
		.find((result) => result.name === "mcp")
		.stdout.split("\n")
		.filter(Boolean)
		.map((line) => JSON.parse(line));
	const mcpRemember = mcpMessages.find((message) => message.id === 2);
	assert.ok(mcpRemember?.result && !mcpRemember.error, "MCP remember response missing or failed");
	assert.equal(JSON.parse(mcpRemember.result.content[0].text).status, "queued");
	assert.equal(JSON.parse(results.find((result) => result.name === "cli").stdout).status, "queued");

	const spool = core.resolveSpoolLayout(dataDir);
	const ready = readdirSync(spool.readyDir);
	assert.equal(ready.length, 4, `expected four surface spool entries, got ${ready.length}`);
	const entries = [];
	for (const name of ready) {
		const path = join(spool.readyDir, name);
		const descriptor = openSync(path, "r");
		try {
			const content = readFileSync(descriptor);
			assert.equal(fstatSync(descriptor).mode & 0o777, 0o600, `${name} is not owner-only`);
			assertNoNeedles(content, [secret, privateText, "<private>"], name);
			entries.push(JSON.parse(content.toString("utf8")));
		} finally {
			closeSync(descriptor);
		}
	}
	const surfaceEvents = entries.filter((entry) => entry.method === "POST /v1/events");
	assert.equal(surfaceEvents.length, 3);
	assert.equal(entries.filter((entry) => entry.method === "POST /v1/memories/record").length, 1);
	const cliEvent = surfaceEvents.find(
		(entry) => entry.body.event.eventId === "phase1-t055-cli-event-1",
	);
	assert.ok(cliEvent, "CLI event identity missing");
	assert.deepEqual(
		new Set(
			surfaceEvents
				.filter((entry) => entry !== cliEvent)
				.map((entry) => entry.body.event.nativeSessionId),
		),
		new Set(["phase1-t055-hook-ingest", "phase1-t055-hook-inject"]),
	);
	if (cliEvent.body.event.nativeSessionId !== "phase1-t055-cli") {
		assert.match(cliEvent.body.event.nativeSessionId, /^redacted:[a-f0-9]{32}$/);
		assert.equal(cliEvent.redaction.redaction_degraded, true);
	}

	const worker = startDaemonWorker(dataDir, env);
	await worker.ready;
	await stopWorker(worker);
	assertNoNeedles(`${worker.stdout}\n${worker.stderr}`, [secret, privateText], "daemon output");
	assert.deepEqual(readdirSync(spool.readyDir), [], "surface spool was not drained");
	const db = new Database(realpathSync(core.resolveStorageLayout(dataDir).currentPointerPath), {
		readonly: true,
	});
	try {
		assert.equal(db.prepare("SELECT COUNT(*) AS n FROM mutation_receipts").get().n, 4);
		assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events").get().n, 3);
		assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memory_items").get().n, 1);
		assert.equal(db.prepare("SELECT COUNT(*) AS n FROM mutation_quarantine").get().n, 0);
	} finally {
		db.close();
	}
	scanTree(fixtureRoot, [secret, privateText, "<private>"]);
	console.log("PASS surface spool/import/privacy: 4 independent built surfaces, exactly once");
}

async function blackholeProxy(socketPath, expectedResponses) {
	const upstreamPath = `${socketPath}.upstream`;
	renameSync(socketPath, upstreamPath);
	let responses = 0;
	const heldConnections = new Set();
	const server = createServer((client) => {
		const upstream = createConnection(upstreamPath);
		const connection = { client, upstream, responded: false };
		heldConnections.add(connection);
		client.on("error", () => {});
		upstream.on("error", () => client.destroy());
		client.pipe(upstream);
		let buffer = Buffer.alloc(0);
		upstream.on("data", (chunk) => {
			buffer = Buffer.concat([buffer, chunk]);
			if (!connection.responded && buffer.includes(0x0a)) {
				connection.responded = true;
				responses++;
				upstream.destroy();
			}
		});
	});
	server.on("error", () => {});
	await new Promise((resolveListen, rejectListen) => {
		server.once("error", rejectListen);
		server.listen(socketPath, resolveListen);
	});
	chmodSync(socketPath, 0o600);
	const release = () => {
		for (const connection of heldConnections) {
			connection.client.destroy();
			connection.upstream.destroy();
		}
		heldConnections.clear();
	};
	return {
		wait: () => waitUntil(`${expectedResponses} withheld RPC responses`, () => responses === expectedResponses),
		release,
		async close() {
			release();
			await new Promise((resolveClose) => server.close(resolveClose));
			for (const path of [socketPath, upstreamPath]) {
				try {
					unlinkSync(path);
				} catch {
					// Socket may already be gone.
				}
			}
		},
	};
}

async function classAReplayGate(core, root) {
	const fixtureRoot = join(root, "class-a");
	const dataDir = join(fixtureRoot, "data");
	const workspace = join(fixtureRoot, "workspace");
	mkdirSync(workspace, { recursive: true, mode: 0o700 });
	const env = runtimeEnv(dataDir, fixtureRoot);
	let worker = startDaemonWorker(dataDir, env);
	let ready = await worker.ready;
	const proxy = await blackholeProxy(ready.socketPath, 2);
	const mcpInput = [
		{
			jsonrpc: "2.0",
			id: 1,
			method: "initialize",
			params: {
				protocolVersion: "2025-11-25",
				capabilities: {},
				clientInfo: { name: "phase1-t055-response-loss", version: "1" },
			},
		},
		{ jsonrpc: "2.0", method: "notifications/initialized", params: {} },
		{
			jsonrpc: "2.0",
			id: 2,
			method: "tools/call",
			params: {
				name: "memory_remember",
				arguments: {
					kind: "decision",
					title: "response lost",
					body: "commit survives response loss",
					project: "phase1-t055",
				},
			},
		},
	]
		.map((message) => JSON.stringify(message))
		.join("\n");
	const lostCalls = [
		startCaptured(
			"response-loss-hook",
			[hookRuntimePath, "codex-hook-ingest"],
			JSON.stringify({
				hook_event_name: "UserPromptSubmit",
				session_id: "phase1-t055-response-loss",
				prompt: "commit survives response loss",
				cwd: workspace,
				timestamp: "2026-08-15T01:05:00.000Z",
			}),
			env,
			workspace,
		),
		startCaptured("response-loss-mcp", [mcpPath], `${mcpInput}\n`, env, workspace),
	];
	await proxy.wait();
	await pauseWorker(worker);
	proxy.release();
	const lostResults = await Promise.all(lostCalls);
	for (const result of lostResults) {
		assert.equal(result.code, 0, `${result.name} failed: ${result.stderr.trim()}`);
		assert.equal(result.timedOut, false, `${result.name} timed out`);
	}
	assert.deepEqual(JSON.parse(lostResults[0].stdout.trim()), { continue: true });
	const lostEntries = readdirSync(core.resolveSpoolLayout(dataDir).readyDir).map((name) =>
		JSON.parse(readFileSync(join(core.resolveSpoolLayout(dataDir).readyDir, name), "utf8")),
	);
	assert.equal(lostEntries.length, 2, "actual response-loss clients did not spool twice");
	const responseLostEventEntry = lostEntries.find((entry) => entry.method === "POST /v1/events");
	const responseLostMemoryEntry = lostEntries.find(
		(entry) => entry.method === "POST /v1/memories/record",
	);
	assert.ok(responseLostEventEntry && responseLostMemoryEntry, "response-loss spool methods missing");
	const responseLostEvent = responseLostEventEntry.body;
	const responseLostMemory = responseLostMemoryEntry.body;
	await killWorker(worker);
	await proxy.close();

	worker = startDaemonWorker(dataDir, env);
	ready = await worker.ready;
	assert.deepEqual(readdirSync(core.resolveSpoolLayout(dataDir).readyDir), []);
	const selectedMemoryId = (
		await rpc(core, ready.socketPath, "POST /v1/memories/record", {
			...responseLostMemory,
			adapterRedaction: responseLostMemoryEntry.redaction,
		})
	).memoryId;
	const rememberBody = {
		idempotencyKey: "x10-remember",
		kind: "decision",
		title: "class A replay",
		body: "one semantic memory",
		project: "phase1-t055",
	};
	let memoryId;
	const eventBody = { idempotencyKey: "x10-event", event: normalizedEvent("x10-event") };
	const batchBody = {
		items: [
			{ idempotencyKey: "x10-batch-a", event: normalizedEvent("x10-batch-a") },
			{ idempotencyKey: "x10-batch-b", event: normalizedEvent("x10-batch-b") },
		],
	};
	const packBody = { requestId: "x10-pack", context: "phase 1 fault injection", limit: 1 };
	const searchBody = { requestId: "x10-search", mode: "recent", limit: 1 };
	const attemptId = randomUUID();
	let retrievalBody;
	for (let attempt = 0; attempt < 10; attempt++) {
		memoryId = (await rpc(core, ready.socketPath, "POST /v1/memories/record", rememberBody)).memoryId;
		retrievalBody ??= {
			attemptId,
			startedAt: "2026-08-15T01:00:00.000Z",
			completedAt: "2026-08-15T01:00:00.010Z",
			retrievalStatus: "succeeded",
			candidateIds: [selectedMemoryId],
			candidateCount: 1,
			selectedIds: [selectedMemoryId],
			project: "phase1-t055",
		};
		await rpc(core, ready.socketPath, "POST /v1/events", eventBody);
		await rpc(core, ready.socketPath, "POST /v1/events/batch", batchBody);
		await rpc(core, ready.socketPath, "POST /v1/context/pack", packBody);
		await rpc(core, ready.socketPath, "POST /v1/search", searchBody);
		await rpc(core, ready.socketPath, "POST /v1/retrieval/file-context", retrievalBody);
		await rpc(core, ready.socketPath, "POST /v1/retrieval/file-context/delivery", {
			attemptId,
			status: "handed_off",
		});
		await rpc(core, ready.socketPath, "GET /v1/memories/:id", {
			id: memoryId,
			requestId: "x10-get",
		});
		await rpc(core, ready.socketPath, "DELETE /v1/memories/:id", {
			id: memoryId,
			requestId: "x10-delete",
		});
	}
	await stopWorker(worker);
	const db = new Database(currentDatabasePath(core, dataDir), { readonly: true });
	try {
		const receiptKeys = [
			["POST /v1/events", responseLostEventEntry.idempotencyKey],
			["POST /v1/memories/record", responseLostMemoryEntry.idempotencyKey],
			["POST /v1/memories/record", "x10-remember"],
			["POST /v1/events", "x10-event"],
			["POST /v1/events", "x10-batch-a"],
			["POST /v1/events", "x10-batch-b"],
			["POST /v1/context/pack", "x10-pack"],
			["POST /v1/search", "x10-search"],
			["GET /v1/memories/:id", "x10-get"],
			["DELETE /v1/memories/:id", "x10-delete"],
		];
		for (const [method, key] of receiptKeys) {
			const row = db
				.prepare(
					"SELECT COUNT(*) AS n, COUNT(DISTINCT receipt_id) AS ids, COUNT(DISTINCT payload_hash) AS hashes FROM mutation_receipts WHERE method = ? AND idempotency_key = ?",
				)
				.get(method, key);
			assert.deepEqual(row, { n: 1, ids: 1, hashes: 1 }, `${method}/${key} duplicated`);
		}
		assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events").get().n, 4);
		assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memory_items").get().n, 2);
		assert.deepEqual(
			db.prepare("SELECT active, rev FROM memory_items WHERE id = ?").get(memoryId),
			{ active: 0, rev: 2 },
		);
		assert.deepEqual(
			db
				.prepare(
					"SELECT retrieval_status, delivery_status FROM retrieval_attempts WHERE attempt_id = ?",
				)
				.get(attemptId),
			{ retrieval_status: "succeeded", delivery_status: "handed_off" },
		);
		assert.deepEqual(
			db
				.prepare(
					"SELECT COUNT(*) AS n, MIN(handoff_status) AS handoff FROM retrieval_exposures WHERE attempt_id = ?",
				)
				.get(attemptId),
			{ n: 1, handoff: "handed_off" },
		);
		assert.equal(db.prepare("SELECT COUNT(*) AS n FROM mutation_quarantine").get().n, 0);
	} finally {
		db.close();
	}
	console.log("PASS Class A: all 9 methods x10; lost response -> spool -> kill -> one commit");
}

function inodeKey(path) {
	const info = statSync(path, { bigint: true });
	return `${info.dev}:${info.ino}`;
}

function fdInodes(pid) {
	const result = new Set();
	for (const descriptor of readdirSync(`/proc/${pid}/fd`)) {
		try {
			const info = statSync(`/proc/${pid}/fd/${descriptor}`, { bigint: true });
			result.add(`${info.dev}:${info.ino}`);
		} catch {
			// Descriptor may close during the scan.
		}
	}
	return result;
}

function processAlive(pid) {
	try {
		const value = readFileSync(`/proc/${pid}/stat`, "utf8");
		const state = value.slice(value.lastIndexOf(")") + 2).split(" ")[0];
		return state !== "Z" && state !== "X";
	} catch {
		return false;
	}
}

async function lifecycleGate(core, root) {
	const raceRoot = join(root, "lifecycle-race");
	const dataDir = join(raceRoot, "data");
	const bootstrap = startDaemonWorker(dataDir, runtimeEnv(dataDir, raceRoot));
	await bootstrap.ready;
	await stopWorker(bootstrap);
	const env = runtimeEnv(dataDir, raceRoot, { T055_SPAWN_SLEEP: "1" });
	let winner;
	let winnerReady;
	for (let attempt = 0; attempt < 20; attempt += 1) {
		const contenders = [
			startDaemonWorker(dataDir, env, { waitForStart: true }),
			startDaemonWorker(dataDir, env, { waitForStart: true }),
		];
		for (const contender of contenders) contender.start();
		const outcomes = await Promise.allSettled(contenders.map((contender) => contender.ready));
		const raceDiagnostics = outcomes.map((outcome) =>
			outcome.status === "fulfilled"
				? `ready:${outcome.value.pid}`
				: `error:${outcome.reason.message}`,
		);
		assert.equal(
			outcomes.filter((outcome) => outcome.status === "fulfilled").length,
			1,
			`daemon race ${attempt + 1} did not produce one owner: ${raceDiagnostics.join(" | ")}`,
		);
		assert.equal(
			outcomes.filter((outcome) => outcome.status === "rejected").length,
			1,
			`daemon race ${attempt + 1} did not reject one contender: ${raceDiagnostics.join(" | ")}`,
		);
		const winnerIndex = outcomes.findIndex((outcome) => outcome.status === "fulfilled");
		winner = contenders[winnerIndex];
		winnerReady = outcomes[winnerIndex].value;
		if (attempt < 19) await stopWorker(winner);
	}
	assert.ok(winnerReady.pid > 1 && winnerReady.pid !== process.pid);
	assert.ok(winnerReady.sleeperPid > 1, "daemon worker did not create the child-process probe");
	await rpc(core, winnerReady.socketPath, "GET /v1/health", {});
	const layout = core.resolveStorageLayout(dataDir);
	const childDescriptors = fdInodes(winnerReady.sleeperPid);
	for (const path of [layout.lockPath, realpathSync(layout.currentPointerPath)]) {
		assert.equal(childDescriptors.has(inodeKey(path)), false, `child inherited ${path}`);
	}
	await killWorker(winner);
	assert.equal(processAlive(winnerReady.sleeperPid), true, "child probe did not survive daemon crash");
	assert.equal(existsSync(layout.socketPath), true, "SIGKILL did not leave the stale socket fixture");
	const restarted = startDaemonWorker(dataDir, runtimeEnv(dataDir, raceRoot));
	const restartedReady = await restarted.ready;
	assert.notEqual(restartedReady.pid, winnerReady.pid);
	await rpc(core, restartedReady.socketPath, "GET /v1/health", {});
	await stopWorker(restarted);
	process.kill(winnerReady.sleeperPid, "SIGKILL");

	const killRoot = join(root, "force-kill");
	const killDataDir = join(killRoot, "data");
	const killLayout = core.resolveStorageLayout(killDataDir);
	core.ensureStorageLayout(killLayout);
	const victim = spawn("/bin/sleep", ["30"], { stdio: "ignore" });
	const successor = spawn("/bin/sleep", ["30"], { stdio: "ignore" });
	assert.ok(victim.pid > 1 && successor.pid > 1);
	try {
		const victimLive = core.readProcessIdentity(victim.pid);
		const victimIdentity = {
			version: 1,
			pid: victim.pid,
			startTime: victimLive.startTime,
			fingerprint: victimLive.fingerprint,
			nonce: "victim-nonce",
		};
		writeFileSync(
			killLayout.identityPath,
			`${JSON.stringify({ ...victimIdentity, startTime: "0", fingerprint: "stale" })}\n`,
			{ mode: 0o600 },
		);
		await assert.rejects(core.forceKillDaemon(killDataDir), /identity|mismatch|refuse/i);
		assert.equal(processAlive(victim.pid), true);
		const successorLive = core.readProcessIdentity(successor.pid);
		writeFileSync(
			killLayout.identityPath,
			`${JSON.stringify({
				version: 1,
				pid: successor.pid,
				startTime: successorLive.startTime,
				fingerprint: successorLive.fingerprint,
				nonce: "successor-nonce",
			})}\n`,
			{ mode: 0o600 },
		);
		await assert.rejects(
			core.forceKillDaemon(killDataDir, victimIdentity),
			/identity|mismatch|refuse/i,
		);
		assert.equal(processAlive(victim.pid), true);
		assert.equal(processAlive(successor.pid), true);
	} finally {
		victim.kill("SIGKILL");
		successor.kill("SIGKILL");
	}

	const stale = spawn("/bin/sleep", ["30"], { stdio: "ignore" });
	assert.ok(stale.pid > 1);
	const staleLive = core.readProcessIdentity(stale.pid);
	writeFileSync(
		killLayout.identityPath,
		`${JSON.stringify({
			version: 1,
			pid: stale.pid,
			startTime: staleLive.startTime,
			fingerprint: staleLive.fingerprint,
			nonce: "dead-record",
		})}\n`,
		{ mode: 0o600 },
	);
	stale.kill("SIGKILL");
	await waitUntil("stale PID exit", () => !processAlive(stale.pid));
	await assert.rejects(core.forceKillDaemon(killDataDir), /identity|mismatch|refuse/i);

	const startupRoot = join(root, "startup-kill");
	const startupDataDir = join(startupRoot, "data");
	mkdirSync(startupRoot, { recursive: true, mode: 0o700 });
	const fifo = join(startupRoot, "trace.fifo");
	const mkfifo = spawnSync("/usr/bin/mkfifo", [fifo], { encoding: "utf8" });
	assert.equal(mkfifo.status, 0, `mkfifo failed: ${mkfifo.stderr}`);
	let firstTrace = "";
	let traceReaderStderr = "";
	const traceReader = spawn("/usr/bin/head", ["-n", "1", fifo], {
		stdio: ["ignore", "pipe", "pipe"],
	});
	trackedChildren.add(traceReader);
	let resolveFirstTrace;
	let rejectFirstTrace;
	const firstTraceReceived = new Promise((resolve, reject) => {
		resolveFirstTrace = resolve;
		rejectFirstTrace = reject;
	});
	let firstTraceSeen = false;
	traceReader.stdout.on("data", (chunk) => {
		firstTrace = appendOutput(firstTrace, chunk, traceReader, "startup trace reader stdout");
		if (!firstTraceSeen) {
			firstTraceSeen = true;
			resolveFirstTrace();
		}
	});
	traceReader.once("error", rejectFirstTrace);
	traceReader.stderr.on("data", (chunk) => {
		traceReaderStderr = appendOutput(
			traceReaderStderr,
			chunk,
			traceReader,
			"startup trace reader stderr",
		);
	});
	const traceReaderDone = new Promise((resolveDone, rejectDone) => {
		traceReader.once("error", rejectDone);
		traceReader.once("exit", (code, signal) => {
			trackedChildren.delete(traceReader);
			resolveDone({ code, signal });
		});
	});
	const startupWorker = startDaemonWorker(
		startupDataDir,
		runtimeEnv(startupDataDir, startupRoot, { CODEMEM_DB_OPEN_TRACE: fifo }),
	);
	const startupLayout = core.resolveStorageLayout(startupDataDir);
	const traceTimeout = sleep(15_000).then(() => null);
	const checkpoint = await Promise.race([
		firstTraceReceived.then(
			() => ({ kind: "trace" }),
			(error) => ({ kind: "trace_error", error }),
		),
		startupWorker.ready.then(
			() => ({ kind: "unexpected_ready" }),
			(error) => ({ kind: "startup_error", error }),
		),
		traceTimeout.then(() => ({ kind: "timeout" })),
	]);
	if (checkpoint.kind !== "trace") {
		traceReader.kill("SIGKILL");
		await traceReaderDone.catch(() => {});
		let detail = checkpoint.kind;
		if (checkpoint.kind === "startup_error" || checkpoint.kind === "trace_error") {
			detail =
				checkpoint.error instanceof Error ? checkpoint.error.message : String(checkpoint.error);
		}
		throw new Error(
			`Startup trace checkpoint failed (${detail}): ${startupWorker.stderr.trim()}`,
		);
	}
	const traceReaderResult = await Promise.race([traceReaderDone, traceTimeout]);
	if (traceReaderResult === null) {
		traceReader.kill("SIGKILL");
		await traceReaderDone.catch(() => {});
		throw new Error("Startup trace reader did not exit before the checkpoint deadline.");
	}
	assert.deepEqual(
		traceReaderResult,
		{ code: 0, signal: null },
		`startup trace reader failed: ${traceReaderStderr.trim()}`,
	);
	const firstTraceEvent = JSON.parse(firstTrace.trim());
	assert.equal(firstTraceEvent.dbPath, startupLayout.capabilityLifecycleLockPath);
	await waitUntil("startup lock before trace FIFO", () => {
		if (!existsSync(startupLayout.lockPath)) return false;
		return fdInodes(startupWorker.child.pid).has(inodeKey(startupLayout.lockPath));
	});
	assert.equal(existsSync(startupLayout.identityPath), false);
	assert.equal(existsSync(startupLayout.socketPath), false);
	await killWorker(startupWorker);
	const recovered = startDaemonWorker(startupDataDir, runtimeEnv(startupDataDir, startupRoot));
	const recoveredReady = await recovered.ready;
	await rpc(core, recoveredReady.socketPath, "GET /v1/health", {});
	await stopWorker(recovered);
	console.log("PASS lifecycle: race, crash restart, stale socket, child FD, force-kill, startup kill");
}

function readJson(path) {
	return JSON.parse(readFileSync(path, "utf8"));
}

function outputTemps(path) {
	const prefix = `${basename(path)}.`;
	return readdirSync(dirname(path)).filter((name) => name.startsWith(prefix) && name.endsWith(".tmp"));
}

async function waitForOperation(core, socketPath, operationId, timeoutMs = 30_000) {
	return waitUntil(
		`operation ${operationId}`,
		async () => {
			const result = await rpc(core, socketPath, "GET /v1/operations/:id", { id: operationId });
			return result.state === "committed" || result.state === "failed" ? result : false;
		},
		timeoutMs,
	);
}

async function classBReplayGate(core, root) {
	const fixtureRoot = join(root, "class-b");
	const dataDir = join(fixtureRoot, "data");
	const env = runtimeEnv(dataDir, fixtureRoot);
	let worker = startDaemonWorker(dataDir, env);
	let ready = await worker.ready;
	await rpc(core, ready.socketPath, "POST /v1/memories/record", {
		idempotencyKey: "class-b-seed",
		kind: "decision",
		title: "class B seed",
		body: "seed",
		project: "phase1-t055",
	});
	await stopWorker(worker);

	const canonicalPath = currentDatabasePath(core, dataDir);
	const fixtureDb = new Database(canonicalPath);
	try {
		const sessionId = fixtureDb.prepare("SELECT id FROM sessions ORDER BY id LIMIT 1").get().id;
		const insert = fixtureDb.prepare(
			"INSERT INTO memory_items(session_id, kind, title, body_text, active, created_at, updated_at, project) VALUES (?, 'decision', ?, ?, 1, ?, ?, 'phase1-t055')",
		);
		const body = "x".repeat(512 * 1024);
		const now = "2026-08-15T02:00:00.000Z";
		fixtureDb.transaction(() => {
			for (let index = 0; index < 64; index++) {
				insert.run(sessionId, `class-b-padding-${index}`, body, now, now);
			}
			fixtureDb.exec(
				"CREATE TABLE t055_backup_padding(payload BLOB NOT NULL); INSERT INTO t055_backup_padding VALUES (zeroblob(100663296));",
			);
		})();
		fixtureDb.pragma("wal_checkpoint(TRUNCATE)");
	} finally {
		fixtureDb.close();
	}

	worker = startDaemonWorker(dataDir, env);
	ready = await worker.ready;
	const layout = core.resolveStorageLayout(dataDir);
	const backupId = "phase1-t055-crash-backup";
	const reason = "Phase 1 T055 mid-flight crash";
	const backupBody = {
		operationId: backupId,
		payloadHash: core.backupPayloadHash(reason),
		reason,
	};
	const interruptedBackup = rpc(
		core,
		ready.socketPath,
		"POST /v1/backup/create",
		backupBody,
	).catch((error) => error);
	await waitUntil(
		"backup temporary artifact",
		() => readdirSync(layout.backupsDir).some((name) => name.startsWith(`${backupId}.`) && name.endsWith(".tmp")),
		30_000,
	);
	assert.equal(existsSync(join(layout.backupsDir, `${backupId}.json`)), false);
	await pauseWorker(worker);
	assert.equal(
		existsSync(join(layout.backupsDir, `${backupId}.tmp`)),
		true,
		"backup crash fixture missed the temporary artifact",
	);
	assert.equal(existsSync(join(layout.backupsDir, `${backupId}.json`)), false);
	await killWorker(worker);
	assert.ok((await interruptedBackup) instanceof Error, "interrupted backup unexpectedly returned success");

	worker = startDaemonWorker(dataDir, env);
	ready = await worker.ready;
	const backup = await rpc(core, ready.socketPath, "POST /v1/backup/create", backupBody);
	assert.deepEqual(
		{
			operationId: backup.operationId,
			backupId: backup.backupId,
			state: backup.state,
		},
		{ operationId: backupId, backupId, state: "completed" },
	);
	const verification = await rpc(core, ready.socketPath, "POST /v1/backup/verify", { backupId });
	assert.equal(verification.valid, true, JSON.stringify(verification.diagnostics));
	assert.equal(verification.manifestHash, backup.manifestHash);
	assert.equal(
		readdirSync(layout.backupsDir).filter((name) => name === `${backupId}.sqlite`).length,
		1,
	);
	assert.equal(
		readdirSync(layout.backupsDir).filter((name) => name === `${backupId}.json`).length,
		1,
	);
	assert.equal(
		readdirSync(layout.backupsDir).filter((name) => name.startsWith(`${backupId}.`) && name.endsWith(".tmp"))
			.length,
		0,
		"backup replay retained a private temporary artifact",
	);
	const backupDb = new Database(join(layout.backupsDir, `${backupId}.sqlite`), { readonly: true });
	try {
		assert.equal(backupDb.prepare("SELECT COUNT(*) AS n FROM memory_items").get().n, 65);
	} finally {
		backupDb.close();
	}

	const operationId = "phase1-t055-export-crash";
	const outputPath = join(fixtureRoot, "crashed-export.json");
	const exportRequest = { outputPath, filters: { allProjects: true } };
	const interruptedExport = rpc(core, ready.socketPath, "POST /v1/operations/export", {
		operationId,
		payloadHash: core.hashMutationPayload(exportRequest),
		...exportRequest,
	}).catch((error) => error);
	const journalPath = join(layout.controlDir, "operations", `${operationId}.json`);
	await waitUntil("export writing temporary artifact", () => {
		return (
			existsSync(journalPath) && readJson(journalPath).state === "writing" && outputTemps(outputPath).length > 0
		);
	});
	await pauseWorker(worker);
	assert.equal(readJson(journalPath).state, "writing");
	assert.ok(outputTemps(outputPath).length > 0, "export crash fixture missed the temporary artifact");
	await killWorker(worker);
	await interruptedExport;
	if (existsSync(outputPath)) JSON.parse(readFileSync(outputPath, "utf8"));

	worker = startDaemonWorker(dataDir, env);
	ready = await worker.ready;
	assert.deepEqual(outputTemps(outputPath), [], "restart retained a private export temporary artifact");
	const failed = await rpc(core, ready.socketPath, "GET /v1/operations/:id", { id: operationId });
	assert.deepEqual(
		{ state: failed.state, code: failed.error?.code },
		{ state: "failed", code: "daemon_restarted" },
	);
	assert.deepEqual(
		await rpc(core, ready.socketPath, "POST /v1/operations/export", {
			operationId,
			payloadHash: core.hashMutationPayload(exportRequest),
			...exportRequest,
		}),
		{ operationId, state: "failed" },
	);
	const retryId = "phase1-t055-export-retry";
	const retryRequest = {
		outputPath: join(fixtureRoot, "retry-export.json"),
		filters: { allProjects: true },
	};
	await rpc(core, ready.socketPath, "POST /v1/operations/export", {
		operationId: retryId,
		payloadHash: core.hashMutationPayload(retryRequest),
		...retryRequest,
	});
	const retried = await waitForOperation(core, ready.socketPath, retryId, 60_000);
	assert.equal(retried.state, "committed");
	assert.equal(retried.result.outputSha256, core.sha256File(retryRequest.outputPath));
	await stopWorker(worker);

	const missingParentId = "phase1-t055-export-missing-parent";
	const missingParentRequest = {
		outputPath: join(fixtureRoot, "removed-export-parent", "export.json"),
		filters: { allProjects: true },
	};
	writeFileSync(
		join(layout.controlDir, "operations", `${missingParentId}.json`),
		`${JSON.stringify({
			version: 1,
			operationId: missingParentId,
			payloadHash: core.hashMutationPayload(missingParentRequest),
			kind: "export",
			state: "writing",
			request: missingParentRequest,
			result: null,
			error: null,
			createdAt: "2026-08-15T02:30:00.000Z",
			updatedAt: "2026-08-15T02:30:00.000Z",
		})}\n`,
		{ mode: 0o600 },
	);
	worker = startDaemonWorker(dataDir, env);
	ready = await worker.ready;
	const cleanupUnverified = await rpc(core, ready.socketPath, "GET /v1/operations/:id", {
		id: missingParentId,
	});
	assert.equal(cleanupUnverified.state, "failed");
	assert.equal(cleanupUnverified.error.code, "daemon_restarted");
	assert.match(cleanupUnverified.error.message, /cleanup could not be verified/);
	await stopWorker(worker);
	console.log("PASS Class B: backup mid-flight crash/replay and export writing crash/safe retry");
}

async function daemonChild(core, dataDir) {
	if (process.env.T055_WAIT_FOR_START === "1") {
		await new Promise((resolveStart) => process.once("message", resolveStart));
	}
	let handle;
	let sleeper;
	try {
		handle = await core.startDaemon({ dataDir });
		if (process.env.T055_SPAWN_SLEEP === "1") {
			sleeper = spawn("/bin/sleep", ["30"], { stdio: "ignore" });
		}
		process.send?.({
			type: "ready",
			pid: process.pid,
			socketPath: handle.socketPath,
			identity: handle.identity,
			sleeperPid: sleeper?.pid ?? null,
		});
	} catch (error) {
		process.send?.({ type: "error", message: error instanceof Error ? error.message : String(error) });
		process.exitCode = 1;
		return;
	}
	process.on("message", async (message) => {
		if (message?.type !== "stop") return;
		try {
			await handle.stop();
		} finally {
			sleeper?.kill("SIGKILL");
			process.exit(0);
		}
	});
}

function verifyPrerequisites() {
	assert.equal(process.platform, "linux", "T055 requires Linux /proc and Unix sockets");
	for (const path of [
		corePath,
		cliPath,
		hookRuntimePath,
		mcpPath,
		"/usr/bin/head",
		"/usr/bin/mkfifo",
	]) {
		assert.ok(existsSync(path), `required T055 artifact missing: ${path}`);
	}
}

async function main() {
	verifyPrerequisites();
	const core = await import(pathToFileURL(corePath).href);
	const root = mkdtempSync(join(tmpdir(), "codemem-phase1-t055-"));
	try {
		await surfacePrivacyGate(core, root);
		await classAReplayGate(core, root);
		await lifecycleGate(core, root);
		await classBReplayGate(core, root);
		console.log("PASS T055 fault injection exit gate");
	} finally {
		for (const child of trackedChildren) {
			try {
				child.kill("SIGKILL");
			} catch {
				// Best effort after a failed assertion.
			}
		}
		rmSync(root, { recursive: true, force: true });
	}
}

if (process.argv[2] === "__daemon") {
	const core = await import(pathToFileURL(corePath).href);
	await daemonChild(core, process.argv[3]);
} else {
	await main();
}
