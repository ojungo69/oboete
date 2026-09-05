#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
	chmodSync,
	copyFileSync,
	cpSync,
	existsSync,
	linkSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readdirSync,
	readlinkSync,
	rmSync,
	statSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const repoRoot = resolve(import.meta.dirname, "..");
const packageRoot = join(repoRoot, "vendor/codemem");
const scriptPath = resolve(import.meta.filename);
const corePath = join(packageRoot, "packages/core/dist/index.js");
const cliPath = join(packageRoot, "packages/cli/dist/index.js");
const hookRuntimePath = join(packageRoot, "packages/cli/dist/hook-runtime.js");
const mcpPath = join(packageRoot, "packages/mcp-server/dist/stdio.js");
const viewerPath = join(packageRoot, "packages/viewer-server/dist/index.js");
const rigScript = join(repoRoot, "harness/rig/rig.sh");
const lsofPath = "/usr/bin/lsof";
const safePath = `${dirname(process.execPath)}:/usr/local/bin:/usr/bin:/bin`;
const require = createRequire(join(packageRoot, "packages/core/package.json"));
const Database = require("better-sqlite3");
const databaseBytes = 64 * 1024 * 1024;
const maxOutputBytes = 1024 * 1024;
const activeCheckpointDeadlineMs = 60_000;
const stateNames = [
	"down",
	"up",
	"maintenance",
	"backup",
	"migration",
	"legacy-cutover",
	"restore",
];
const specialStates = new Set([
	"maintenance",
	"backup",
	"migration",
	"legacy-cutover",
	"restore",
]);
const trackedChildren = new Set();

function sleep(ms) {
	return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

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

function cappedAppend(current, chunk, child, label) {
	const next = current + String(chunk);
	if (Buffer.byteLength(next, "utf8") <= maxOutputBytes) return next;
	try {
		process.kill(-child.pid, "SIGKILL");
	} catch {
		child.kill("SIGKILL");
	}
	throw new Error(`${label} exceeded the output cap.`);
}

function childFailure(label, child, stderr) {
	return new Error(
		`${label} exited before it was ready (code=${child.exitCode}, signal=${child.signalCode}): ${stderr.trim()}`,
	);
}

function startDaemonWorker(dataDir, env) {
	let stdout = "";
	let stderr = "";
	let readyMessage = null;
	let readySettled = false;
	const child = spawn(process.execPath, [scriptPath, "__daemon", dataDir], {
		cwd: packageRoot,
		env,
		stdio: ["ignore", "pipe", "pipe", "ipc"],
	});
	trackedChildren.add(child);
	child.stdout.on("data", (chunk) => {
		stdout = cappedAppend(stdout, chunk, child, "daemon worker stdout");
	});
	child.stderr.on("data", (chunk) => {
		stderr = cappedAppend(stderr, chunk, child, "daemon worker stderr");
	});
	const ready = new Promise((resolveReady, rejectReady) => {
		child.on("message", (message) => {
			if (message?.type === "ready") {
				readyMessage = message;
				readySettled = true;
				resolveReady(message);
			} else if (message?.type === "error") {
				readySettled = true;
				rejectReady(new Error(`Daemon start failed: ${message.message}`));
			}
		});
		child.once("error", (error) => {
			readySettled = true;
			rejectReady(error);
		});
		child.once("exit", () => {
			trackedChildren.delete(child);
			if (!readySettled) rejectReady(childFailure("daemon worker", child, stderr));
		});
	});
	return {
		child,
		ready,
		get readyMessage() {
			return readyMessage;
		},
		get stderr() {
			return stderr;
		},
	};
}

async function stopDaemonWorker(worker) {
	const { child } = worker;
	if (child.exitCode !== null || child.signalCode !== null) return;
	try {
		process.kill(child.pid, "SIGCONT");
	} catch {
		// The worker may not be stopped.
	}
	const exited = new Promise((resolveExit) => child.once("exit", resolveExit));
	child.send({ type: "stop" });
	let timeout;
	const timedOut = new Promise((resolveTimeout) => {
		timeout = setTimeout(() => resolveTimeout("timeout"), 5_000);
	});
	const result = await Promise.race([exited, timedOut]);
	clearTimeout(timeout);
	if (result === "timeout") child.kill("SIGKILL");
	await exited;
}

async function pauseWorker(worker) {
	assert.ok(worker.child.pid > 1, "daemon PID must be greater than one");
	assert.notEqual(worker.child.pid, process.pid, "daemon must run in an independent PID");
	process.kill(worker.child.pid, "SIGSTOP");
	await waitUntil("daemon SIGSTOP", () => {
		const status = readFileSync(`/proc/${worker.child.pid}/status`, "utf8");
		return /^State:\s+T/m.test(status);
	});
}

function resumeWorker(worker) {
	if (worker.child.exitCode === null && worker.child.signalCode === null) {
		process.kill(worker.child.pid, "SIGCONT");
	}
}

function runtimeEnv(dataDir, tracePath, rigBase) {
	const temp = join(rigBase, "tmp");
	mkdirSync(temp, { recursive: true, mode: 0o700 });
	return {
		PATH: safePath,
		HOME: join(rigBase, "home"),
		XDG_CONFIG_HOME: join(rigBase, "home/.config"),
		TERM: "dumb",
		NO_COLOR: "1",
		TMPDIR: temp,
		AGENT_MEMORY_INTERNAL_RUN: "1",
		CODEMEM_DATA_DIR: dataDir,
		CODEMEM_DB_OPEN_TRACE: tracePath,
		CODEMEM_EMBEDDING_DISABLED: "1",
	};
}

function readTrace(tracePath) {
	if (!existsSync(tracePath)) return [];
	const raw = readFileSync(tracePath, "utf8");
	const complete = raw.endsWith("\n") ? raw : raw.slice(0, raw.lastIndexOf("\n") + 1);
	return complete
		.split("\n")
		.filter(Boolean)
		.map((line) => JSON.parse(line));
}

async function waitForTrace(label, worker, tracePath, predicate) {
	return waitUntil(label, () => {
		if (worker.child.exitCode !== null || worker.child.signalCode !== null) {
			throw childFailure(label, worker.child, worker.stderr);
		}
		return readTrace(tracePath).find(predicate) ?? false;
	});
}

function targetRecord(path, category, label = path, evidencePath = path) {
	if (!existsSync(path)) return null;
	let info;
	try {
		info = statSync(path, { bigint: true });
	} catch {
		return null;
	}
	if (!info.isFile()) return null;
	return {
		path,
		evidencePath,
		category,
		label,
		dev: info.dev,
		ino: info.ino,
		key: `${info.dev}:${info.ino}`,
	};
}

function addTarget(records, record) {
	if (!record) return;
	const existing = records.get(record.key);
	if (existing) {
		assert.equal(existing.category, record.category, `DB inode changed category: ${record.label}`);
		return;
	}
	records.set(record.key, record);
}

function collectTargets(fixture) {
	const records = new Map();
	for (const record of fixture.preservedTargets) addTarget(records, record);
	if (existsSync(fixture.layout.versionsDir)) {
		for (const name of readdirSync(fixture.layout.versionsDir)) {
			if (name.endsWith(".sqlite")) {
				const path = join(fixture.layout.versionsDir, name);
				addTarget(records, targetRecord(path, "canonical", `canonical:${name}`));
			}
		}
	}
	if (existsSync(fixture.layout.backupsDir)) {
		for (const name of readdirSync(fixture.layout.backupsDir)) {
			if (name.endsWith(".sqlite")) {
				const path = join(fixture.layout.backupsDir, name);
				addTarget(records, targetRecord(path, "auxiliary", `backup:${name}`));
			}
		}
	}
	addTarget(records, targetRecord(fixture.layout.lockPath, "auxiliary", "daemon-lock"));
	for (const event of readTrace(fixture.tracePath)) {
		if (typeof event.dbPath !== "string" || !event.dbPath.startsWith(`${fixture.dataDir}/`)) continue;
		const category = event.dbPath.startsWith(`${fixture.layout.versionsDir}/`)
			? "canonical"
			: event.dbPath.startsWith(`${fixture.layout.backupsDir}/`) ||
					event.dbPath === fixture.layout.lockPath
				? "auxiliary"
				: event.dbPath === fixture.legacyPath
					? "legacy"
					: "auxiliary";
		addTarget(records, targetRecord(event.dbPath, category, `trace:${basename(event.dbPath)}`));
	}
	return [...records.values()];
}

function ownUid(pid) {
	try {
		const match = readFileSync(`/proc/${pid}/status`, "utf8").match(/^Uid:\s+(\d+)/m);
		return match ? Number.parseInt(match[1], 10) : null;
	} catch {
		return null;
	}
}

function procOwners(records) {
	const keys = new Set(records.map((record) => record.key));
	const owners = new Map(records.map((record) => [record.key, new Set()]));
	const uid = process.getuid();
	for (const entry of readdirSync("/proc", { withFileTypes: true })) {
		if (!entry.isDirectory() || !/^\d+$/.test(entry.name)) continue;
		const pid = Number.parseInt(entry.name, 10);
		if (ownUid(pid) !== uid) continue;
		let descriptors;
		try {
			descriptors = readdirSync(`/proc/${pid}/fd`);
		} catch (error) {
			if (error?.code === "ENOENT" || error?.code === "EACCES" || error?.code === "EPERM") {
				// Same contract as legacy cutover: trusted lsof covers an incomplete /proc scan.
				continue;
			}
			throw new Error(`Could not scan /proc/${pid}/fd.`, { cause: error });
		}
		for (const descriptor of descriptors) {
			try {
				const info = statSync(`/proc/${pid}/fd/${descriptor}`, { bigint: true });
				const key = `${info.dev}:${info.ino}`;
				if (keys.has(key)) owners.get(key).add(pid);
			} catch {
				// The process may close a descriptor while /proc is scanned.
			}
		}
	}
	return owners;
}

function lsofOwners(records) {
	const owners = new Map();
	for (const record of records) {
		const result = spawnSync(lsofPath, ["-nP", "-Fp", "--", record.evidencePath], {
			encoding: "utf8",
			timeout: 5_000,
		});
		if (result.error) throw result.error;
		if (result.status !== 0 && result.status !== 1) {
			throw new Error(`lsof failed for ${record.label}: ${String(result.stderr).trim()}`);
		}
		owners.set(
			record.key,
			new Set(
				String(result.stdout)
					.split("\n")
					.filter((line) => /^p\d+$/.test(line))
					.map((line) => Number.parseInt(line.slice(1), 10)),
			),
		);
	}
	return owners;
}

function categorySets(records, byIdentity) {
	const result = {
		canonical: new Set(),
		legacy: new Set(),
		auxiliary: new Set(),
	};
	for (const record of records) {
		for (const pid of byIdentity.get(record.key) ?? []) result[record.category].add(pid);
	}
	return Object.fromEntries(
		Object.entries(result).map(([category, pids]) => [category, [...pids].sort((a, b) => a - b)]),
	);
}

function observeOwners(records) {
	return {
		proc: categorySets(records, procOwners(records)),
		lsof: categorySets(records, lsofOwners(records)),
	};
}

function assertOnly(pids, allowedPid, label) {
	assert.ok(
		pids.every((pid) => pid === allowedPid),
		`${label} owner set escaped daemon PID ${allowedPid}: ${pids.join(",") || "none"}`,
	);
}

function assertStateOwners(state, observation, daemonPid) {
	for (const [scanner, sets] of Object.entries(observation)) {
		if (state === "down") {
			for (const [category, pids] of Object.entries(sets)) {
				assert.deepEqual(pids, [], `${state}/${scanner}/${category} owner set must be empty`);
			}
			continue;
		}
		assert.ok(daemonPid > 1 && daemonPid !== process.pid, `${state} daemon PID is not independent`);
		if (state === "up") {
			assert.deepEqual(
				sets.canonical,
				[daemonPid],
				`${state}/${scanner}/canonical owner set must equal daemon PID`,
			);
			assert.deepEqual(sets.legacy, [], `${state}/${scanner}/legacy owner set must be empty`);
			assertOnly(sets.auxiliary, daemonPid, `${state}/${scanner}/auxiliary`);
			continue;
		}
		assert.ok(specialStates.has(state), `Unknown runtime state: ${state}`);
		for (const [category, pids] of Object.entries(sets)) {
			assertOnly(pids, daemonPid, `${state}/${scanner}/${category}`);
		}
		assert.ok(
			sets.canonical.length + sets.legacy.length > 0,
			`${state}/${scanner} must observe a canonical or legacy DB owner`,
		);
	}
}

function startCaptured(name, args, input, env, cwd) {
	let stdout = "";
	let stderr = "";
	let outputError = null;
	let timedOut = false;
	const child = spawn(process.execPath, args, {
		cwd,
		env,
		detached: true,
		stdio: ["pipe", "pipe", "pipe"],
	});
	trackedChildren.add(child);
	child.stdout.on("data", (chunk) => {
		try {
			stdout = cappedAppend(stdout, chunk, child, `${name} stdout`);
		} catch (error) {
			outputError = error;
		}
	});
	child.stderr.on("data", (chunk) => {
		try {
			stderr = cappedAppend(stderr, chunk, child, `${name} stderr`);
		} catch (error) {
			outputError = error;
		}
	});
	child.stdin.end(input);
	const promise = new Promise((resolveResult, rejectResult) => {
		child.once("error", rejectResult);
		const timeout = setTimeout(() => {
			timedOut = true;
			try {
				process.kill(-child.pid, "SIGKILL");
			} catch {
				child.kill("SIGKILL");
			}
		}, 9_000);
		child.once("exit", (code, signal) => {
			clearTimeout(timeout);
			trackedChildren.delete(child);
			if (outputError) rejectResult(outputError);
			else resolveResult({ name, pid: child.pid, code, signal, stdout, stderr, timedOut });
		});
	});
	return { name, pid: child.pid, promise };
}

function parseJsonOutput(result) {
	const output = result.stdout.trim();
	assert.ok(output, `${result.name} produced no JSON output: ${result.stderr.trim()}`);
	return JSON.parse(output);
}

function validateSurface(result) {
	assert.equal(result.timedOut, false, `${result.name} exceeded its outer timeout`);
	assert.equal(result.signal, null, `${result.name} was terminated by ${result.signal}`);
	if (result.name === "hook-ingest" || result.name === "hook-inject") {
		assert.equal(result.code, 0, `${result.name} failed: ${result.stderr.trim()}`);
		assert.equal(parseJsonOutput(result).continue, true, `${result.name} did not fail open`);
		return;
	}
	if (result.name === "viewer") {
		assert.equal(result.code, 0, `viewer probe failed: ${result.stderr.trim()}`);
		assert.equal(typeof parseJsonOutput(result).ok, "boolean");
		return;
	}
	if (result.name === "mcp") {
		assert.equal(result.code, 0, `MCP probe failed: ${result.stderr.trim()}`);
		const messages = result.stdout
			.split("\n")
			.filter(Boolean)
			.map((line) => JSON.parse(line));
		assert.ok(messages.some((message) => message.id === 1), "MCP initialize response is missing");
		assert.ok(messages.some((message) => message.id === 2), "MCP memory_status response is missing");
		return;
	}
	assert.ok(
		result.code === 0 || result.code === 1,
		`${result.name} crashed (code=${result.code}): ${result.stderr.trim()}`,
	);
	if (result.name === "cli") parseJsonOutput(result);
}

async function runSurfaceBatch(label, fixture, daemonPid) {
	const payload = JSON.stringify({
		hook_event_name: "UserPromptSubmit",
		session_id: `phase1-t054-${label}`,
		prompt: `Phase 1 T054 ${label} runtime boundary probe`,
		cwd: fixture.workspace,
		timestamp: "2026-08-14T00:00:00.000Z",
	});
	const mcpInput = [
		{
			jsonrpc: "2.0",
			id: 1,
			method: "initialize",
			params: {
				protocolVersion: "2025-11-25",
				capabilities: {},
				clientInfo: { name: "phase1-t054", version: "1" },
			},
		},
		{ jsonrpc: "2.0", method: "notifications/initialized", params: {} },
		{
			jsonrpc: "2.0",
			id: 2,
			method: "tools/call",
			params: { name: "memory_status", arguments: {} },
		},
	]
		.map((message) => JSON.stringify(message))
		.join("\n");
	const processes = [
		startCaptured(
			"hook-ingest",
			[hookRuntimePath, "codex-hook-ingest"],
			payload,
			fixture.env,
			fixture.workspace,
		),
		startCaptured(
			"hook-inject",
			[hookRuntimePath, "codex-hook-inject"],
			payload,
			fixture.env,
			fixture.workspace,
		),
		startCaptured("mcp", [mcpPath], `${mcpInput}\n`, fixture.env, fixture.workspace),
		startCaptured("cli", [cliPath, "status", "--json"], "", fixture.env, fixture.workspace),
		startCaptured(
			"viewer",
			[scriptPath, "__viewer", fixture.dataDir],
			"",
			fixture.env,
			fixture.workspace,
		),
		startCaptured(
			"jobs",
			[cliPath, "db", "raw-events-status", "--json", "--limit", "1"],
			"",
			fixture.env,
			fixture.workspace,
		),
	];
	const pids = processes.map((processInfo) => processInfo.pid);
	assert.equal(new Set(pids).size, processes.length, `${label} surfaces did not use unique PIDs`);
	for (const pid of pids) {
		assert.ok(pid > 1 && pid !== process.pid && pid !== daemonPid, `${label} surface PID is not independent`);
	}
	const results = await Promise.all(processes.map((processInfo) => processInfo.promise));
	for (const result of results) validateSurface(result);
	return results;
}

function spawnRogue(path, env) {
	let stderr = "";
	let settled = false;
	const child = spawn(process.execPath, [scriptPath, "__rogue", path], {
		cwd: packageRoot,
		env,
		stdio: ["ignore", "ignore", "pipe", "ipc"],
	});
	trackedChildren.add(child);
	child.stderr.on("data", (chunk) => {
		stderr = cappedAppend(stderr, chunk, child, "rogue opener stderr");
	});
	const ready = new Promise((resolveReady, rejectReady) => {
		child.on("message", (message) => {
			if (message?.type === "ready") {
				settled = true;
				resolveReady(child);
			}
		});
		child.once("error", rejectReady);
		child.once("exit", () => {
			trackedChildren.delete(child);
			if (!settled) rejectReady(childFailure("rogue opener", child, stderr));
		});
	});
	return { child, ready };
}

async function stopRogue(rogue) {
	if (rogue.child.exitCode !== null || rogue.child.signalCode !== null) return;
	const exited = new Promise((resolveExit) => rogue.child.once("exit", resolveExit));
	rogue.child.send({ type: "stop" });
	let timeout;
	const timedOut = new Promise((resolveTimeout) => {
		timeout = setTimeout(resolveTimeout, 2_000);
	});
	await Promise.race([exited, timedOut]);
	clearTimeout(timeout);
	if (rogue.child.exitCode === null && rogue.child.signalCode === null) rogue.child.kill("SIGKILL");
	await exited;
}

async function negativeOwnerControl(state, fixture, daemonPid, records, category, path) {
	const rogue = spawnRogue(path, fixture.env);
	await rogue.ready;
	assert.ok(rogue.child.pid > 1 && rogue.child.pid !== daemonPid, `${state} rogue PID is invalid`);
	try {
		const observed = observeOwners(records);
		for (const scanner of ["proc", "lsof"]) {
			assert.ok(
				observed[scanner][category].includes(rogue.child.pid),
				`${state}/${category} ${scanner} did not see the independent rogue PID`,
			);
		}
		assert.throws(
			() => assertStateOwners(state, observed, daemonPid),
			/owner set|must be empty/,
			`${state}/${category} did not fail on a second PID`,
		);
	} finally {
		await stopRogue(rogue);
	}
	await waitUntil(`${state}/${category} rogue close`, () => {
		try {
			assertStateOwners(state, observeOwners(records), daemonPid);
			return true;
		} catch {
			return false;
		}
	});
}

function databaseFingerprint(fixture, core) {
	const candidates = new Set([
		fixture.layout.currentPointerPath,
		fixture.layout.lockPath,
		fixture.legacyPath,
		`${fixture.legacyPath}-wal`,
		`${fixture.legacyPath}-shm`,
	]);
	const visit = (dir) => {
		if (!existsSync(dir)) return;
		for (const entry of readdirSync(dir, { withFileTypes: true })) {
			const path = join(dir, entry.name);
			if (entry.isDirectory()) visit(path);
			else if (entry.name.endsWith(".sqlite") || /-(?:wal|shm)$/.test(entry.name)) {
				candidates.add(path);
			}
		}
	};
	visit(fixture.layout.versionsDir);
	const result = {};
	for (const path of [...candidates].sort()) {
		const key = relative(fixture.dataDir, path);
		if (!existsSync(path)) {
			result[key] = { exists: false };
			continue;
		}
		const info = lstatSync(path, { bigint: true });
		if (info.isSymbolicLink()) {
			result[key] = {
				exists: true,
				kind: "symlink",
				target: readlinkSync(path),
				mtimeNs: String(info.mtimeNs),
			};
		} else if (info.isFile()) {
			result[key] = {
				exists: true,
				kind: "file",
				size: String(info.size),
				mtimeNs: String(info.mtimeNs),
				sha256: core.sha256File(path),
			};
		}
	}
	return result;
}

function validateTrace(state, fixture, daemonPid) {
	const events = readTrace(fixture.tracePath);
	if (state === "down") {
		assert.deepEqual(events, [], "down surfaces opened SQLite through a traced wrapper");
		return;
	}
	assert.ok(events.length >= 2, `${state} emitted no daemon DB-open trace`);
	for (const event of events) {
		assert.equal(event.version, 1, `${state} trace version mismatch`);
		assert.equal(event.event, "sqlite_open", `${state} trace event mismatch`);
		assert.equal(event.pid, daemonPid, `${state} wrapper trace escaped the daemon PID`);
		assert.ok(
			["daemon_lifecycle", "writer_actor", "readonly_actor"].includes(event.owner),
			`${state} trace owner is unknown`,
		);
		assert.ok(event.dbPath.startsWith(`${fixture.dataDir}/`), `${state} trace escaped its data dir`);
	}
	assert.ok(events.some((event) => event.owner === "daemon_lifecycle"), `${state} lock trace is missing`);
	assert.ok(events.some((event) => event.owner === "writer_actor"), `${state} writer trace is missing`);
	if (state === "backup") {
		assert.ok(events.some((event) => event.owner === "readonly_actor"), "backup readonly trace is missing");
	}
	if (state === "legacy-cutover") {
		assert.ok(events.some((event) => event.dbPath === fixture.legacyPath), "legacy cutover trace is missing");
		assert.ok(
			events.some((event) => event.dbPath.startsWith(`${fixture.layout.versionsDir}/`)),
			"cutover canonical trace is missing",
		);
	}
	if (state === "restore") {
		assert.ok(
			events.some(
				(event) => event.owner === "writer_actor" && basename(event.dbPath).startsWith("restore-"),
			),
			"restore staging writer trace is missing",
		);
	}
}

function rpcRequest(core, socketPath, method, body = {}, timeoutMs = 30_000) {
	return core.callDaemonRpc(
		socketPath,
		{
			id: randomUUID(),
			method,
			adapter_version: "phase1-t054",
			native_cli_version: "phase1-t054",
			normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
			local_api_version: core.LOCAL_API_VERSION,
			capability_hash: core.RPC_CAPABILITY_HASH,
			body,
		},
		{ timeoutMs },
	);
}

function rpcResult(response, label) {
	assert.ok(response && !("error" in response), `${label} failed: ${JSON.stringify(response)}`);
	return response.result;
}

function currentCanonicalPath(core, layout) {
	const pointer = core.readCurrentDatabasePointer(layout);
	assert.ok(pointer, `Canonical pointer is missing under ${layout.dataDir}`);
	return join(layout.dbDir, pointer);
}

function hardlinkTarget(source, destination, category, label) {
	mkdirSync(dirname(destination), { recursive: true, mode: 0o700 });
	if (existsSync(destination)) unlinkSync(destination);
	linkSync(source, destination);
	return targetRecord(source, category, label, destination);
}

async function initializeBase(core, root, rigBase) {
	const dataDir = join(root, "base-data");
	const unusedTrace = join(root, "base-init-trace.jsonl");
	const env = runtimeEnv(dataDir, unusedTrace, rigBase);
	delete env.CODEMEM_DB_OPEN_TRACE;
	const worker = startDaemonWorker(dataDir, env);
	await worker.ready;
	await stopDaemonWorker(worker);
	const layout = core.resolveStorageLayout(dataDir);
	const canonicalPath = currentCanonicalPath(core, layout);
	const db = new Database(canonicalPath);
	try {
		db.pragma("journal_mode = WAL");
		db.exec("CREATE TABLE IF NOT EXISTS phase1_runtime_payload (payload BLOB NOT NULL)");
		db.exec(`INSERT INTO phase1_runtime_payload(payload) VALUES (zeroblob(${databaseBytes}))`);
		db.pragma("wal_checkpoint(TRUNCATE)");
	} finally {
		db.close();
	}
	const legacyPath = join(dataDir, "mem.sqlite");
	copyFileSync(canonicalPath, legacyPath);
	chmodSync(legacyPath, 0o600);
	return dataDir;
}

function prepareFixture(core, root, rigBase, baseDataDir, state) {
	const stateRoot = join(root, "states", state);
	const dataDir = join(stateRoot, "data");
	mkdirSync(stateRoot, { recursive: true, mode: 0o700 });
	cpSync(baseDataDir, dataDir, {
		recursive: true,
		preserveTimestamps: true,
		verbatimSymlinks: true,
	});
	const layout = core.resolveStorageLayout(dataDir);
	const legacyPath = join(dataDir, "mem.sqlite");
	let canonicalPath = currentCanonicalPath(core, layout);
	if (state === "migration") {
		const db = new Database(canonicalPath);
		try {
			db.exec("DROP TABLE raw_event_identity_conflicts");
			db
				.prepare("UPDATE schema_compat_state SET applied_schema_version = ? WHERE id = 1")
				.run(core.MIN_WRITABLE_SCHEMA);
			db.pragma(`user_version = ${core.MIN_WRITABLE_SCHEMA}`);
			db.pragma("wal_checkpoint(TRUNCATE)");
		} finally {
			db.close();
		}
	}
	if (state === "legacy-cutover") {
		unlinkSync(layout.currentPointerPath);
		rmSync(layout.versionsDir, { recursive: true, force: true });
		mkdirSync(layout.versionsDir, { recursive: true, mode: 0o700 });
		const managedPath = join(stateRoot, "thin-client.sh");
		writeFileSync(managedPath, "#!/bin/sh\nexec codemem codex-hook-ingest\n", { mode: 0o600 });
		core.writeInstallManifest(layout.installManifestPath, {
			version: 1,
			blocks: [],
			targets: [core.captureManagedTarget("thin-client", managedPath)],
		});
		canonicalPath = null;
	}
	const evidenceDir = join(stateRoot, "evidence-links");
	const preservedTargets = [];
	if (canonicalPath) {
		preservedTargets.push(
			hardlinkTarget(
				canonicalPath,
				join(evidenceDir, "canonical.sqlite"),
				"canonical",
				"canonical-evidence",
			),
		);
	}
	preservedTargets.push(
		hardlinkTarget(
			legacyPath,
			join(evidenceDir, "legacy.sqlite"),
			"legacy",
			"legacy-evidence",
		),
	);
	const tracePath = join(stateRoot, "db-open-trace.jsonl");
	writeFileSync(tracePath, "", { mode: 0o600 });
	return {
		state,
		stateRoot,
		dataDir,
		layout,
		legacyPath,
		canonicalPath,
		preservedTargets: preservedTargets.filter(Boolean),
		tracePath,
		env: runtimeEnv(dataDir, tracePath, rigBase),
		workspace: join(rigBase, "workspace"),
	};
}

async function stoppedBoundary(state, fixture, core, daemonPid) {
	await waitUntil(`${state} stopped owner release`, () => {
		try {
			assertStateOwners("down", observeOwners(collectTargets(fixture)), null);
			return true;
		} catch {
			return false;
		}
	});
	const before = databaseFingerprint(fixture, core);
	const traceCount = readTrace(fixture.tracePath).length;
	await runSurfaceBatch(`${state}-stopped`, fixture, daemonPid);
	assertStateOwners("down", observeOwners(collectTargets(fixture)), null);
	assert.deepEqual(
		databaseFingerprint(fixture, core),
		before,
		`${state} stopped surfaces changed a DB hash, size, mtime, pointer, or sidecar`,
	);
	assert.equal(
		readTrace(fixture.tracePath).length,
		traceCount,
		`${state} stopped surfaces emitted a DB-open trace`,
	);
}

async function exerciseDown(fixture, core) {
	const records = collectTargets(fixture);
	assertStateOwners("down", observeOwners(records), null);
	await negativeOwnerControl("down", fixture, null, records, "canonical", fixture.canonicalPath);
	await negativeOwnerControl("down", fixture, null, records, "legacy", fixture.legacyPath);
	const before = databaseFingerprint(fixture, core);
	await runSurfaceBatch("down", fixture, null);
	assertStateOwners("down", observeOwners(collectTargets(fixture)), null);
	assert.deepEqual(databaseFingerprint(fixture, core), before, "down surfaces changed DB state");
	validateTrace("down", fixture, null);
}

async function exerciseUp(fixture, core) {
	const worker = startDaemonWorker(fixture.dataDir, fixture.env);
	const ready = await worker.ready;
	const pid = ready.pid;
	try {
		let records = collectTargets(fixture);
		assertStateOwners("up", observeOwners(records), pid);
		await negativeOwnerControl("up", fixture, pid, records, "canonical", fixture.canonicalPath);
		await negativeOwnerControl("up", fixture, pid, records, "legacy", fixture.legacyPath);
		await runSurfaceBatch("up", fixture, pid);
		records = collectTargets(fixture);
		assertStateOwners("up", observeOwners(records), pid);
		validateTrace("up", fixture, pid);
	} finally {
		await stopDaemonWorker(worker);
	}
	await stoppedBoundary("up", fixture, core, pid);
}

async function exerciseStartupState(state, fixture, core) {
	const worker = startDaemonWorker(fixture.dataDir, fixture.env);
	const pid = worker.child.pid;
	const tracePredicate =
		state === "migration"
			? (event) => event.owner === "writer_actor" && event.dbPath === fixture.canonicalPath
			: (event) => event.owner === "writer_actor" && event.dbPath === fixture.legacyPath;
	let paused = false;
	try {
		await waitForTrace(`${state} active DB open`, worker, fixture.tracePath, tracePredicate);
		assert.equal(worker.readyMessage, null, `${state} completed before the active checkpoint`);
		await pauseWorker(worker);
		paused = true;
		let records = collectTargets(fixture);
		assertStateOwners(state, observeOwners(records), pid);
		if (state === "migration") {
			await negativeOwnerControl(state, fixture, pid, records, "canonical", fixture.canonicalPath);
		}
		await negativeOwnerControl(state, fixture, pid, records, "legacy", fixture.legacyPath);
		await runSurfaceBatch(state, fixture, pid);
		assertStateOwners(state, observeOwners(collectTargets(fixture)), pid);
		resumeWorker(worker);
		paused = false;
		await worker.ready;
		records = collectTargets(fixture);
		assertStateOwners(state, observeOwners(records), pid);
		if (state === "legacy-cutover") {
			const canonicalPath = currentCanonicalPath(core, fixture.layout);
			await negativeOwnerControl(state, fixture, pid, records, "canonical", canonicalPath);
			assert.ok(lstatSync(fixture.legacyPath).isSymbolicLink(), "legacy tombstone is missing");
		}
		validateTrace(state, fixture, pid);
	} finally {
		if (paused) resumeWorker(worker);
		await stopDaemonWorker(worker);
	}
	if (state === "migration") {
		const db = new Database(fixture.canonicalPath, { readonly: true, fileMustExist: true });
		try {
			assert.equal(db.pragma("user_version", { simple: true }), core.SCHEMA_VERSION);
			assert.ok(
				db
					.prepare(
						"SELECT 1 FROM sqlite_master WHERE type='table' AND name='raw_event_identity_conflicts'",
					)
					.get(),
				"migration did not restore raw_event_identity_conflicts",
			);
		} finally {
			db.close();
		}
	}
	await stoppedBoundary(state, fixture, core, pid);
}

async function exerciseBackup(fixture, core) {
	const worker = startDaemonWorker(fixture.dataDir, fixture.env);
	const ready = await worker.ready;
	const pid = ready.pid;
	const baseline = readTrace(fixture.tracePath).length;
	let done = false;
	const request = rpcRequest(
		core,
		ready.socketPath,
		"POST /v1/backup/create",
		{
			operationId: "phase1-t054-backup",
			reason: "Phase 1 T054 runtime trace",
			payloadHash: core.backupPayloadHash("Phase 1 T054 runtime trace"),
		},
		activeCheckpointDeadlineMs,
	).finally(() => {
		done = true;
	});
	let paused = false;
	try {
		await waitForTrace(
			"backup readonly open",
			worker,
			fixture.tracePath,
			(event, index) => index >= baseline && event.owner === "readonly_actor",
		);
		assert.equal(done, false, "backup completed before the active checkpoint");
		await pauseWorker(worker);
		paused = true;
		const records = collectTargets(fixture);
		assertStateOwners("backup", observeOwners(records), pid);
		await negativeOwnerControl("backup", fixture, pid, records, "canonical", fixture.canonicalPath);
		await negativeOwnerControl("backup", fixture, pid, records, "legacy", fixture.legacyPath);
		await runSurfaceBatch("backup", fixture, pid);
		assertStateOwners("backup", observeOwners(collectTargets(fixture)), pid);
		resumeWorker(worker);
		paused = false;
		rpcResult(await request, "backup");
		validateTrace("backup", fixture, pid);
	} finally {
		if (paused) resumeWorker(worker);
		await stopDaemonWorker(worker);
	}
	await stoppedBoundary("backup", fixture, core, pid);
}

async function waitForJob(core, socketPath, jobId, worker) {
	return waitUntil(`job ${jobId} terminal state`, async () => {
		if (worker.child.exitCode !== null || worker.child.signalCode !== null) {
			throw childFailure(`job ${jobId}`, worker.child, worker.stderr);
		}
		const response = await rpcRequest(core, socketPath, "GET /v1/jobs/:id", { id: jobId }, 1_000);
		const job = rpcResult(response, `job ${jobId}`).job;
		return job?.state === "completed" || job?.state === "failed" ? job : false;
	}, 30_000);
}

async function exerciseMaintenance(fixture, core) {
	const worker = startDaemonWorker(fixture.dataDir, fixture.env);
	const ready = await worker.ready;
	const pid = ready.pid;
	const submitted = rpcResult(
		await rpcRequest(core, ready.socketPath, "POST /v1/jobs", {
			kind: "db.vacuum",
			args: {},
			dryRun: false,
		}),
		"maintenance submit",
	);
	assert.equal(typeof submitted.jobId, "string", "maintenance job ID is missing");
	let paused = false;
	try {
		await waitUntil("maintenance mode", async () => {
			try {
				const response = await rpcRequest(core, ready.socketPath, "GET /v1/health", {}, 250);
				return rpcResult(response, "maintenance health").maintenanceMode === true;
			} catch {
				return false;
			}
		});
		await pauseWorker(worker);
		paused = true;
		const records = collectTargets(fixture);
		assertStateOwners("maintenance", observeOwners(records), pid);
		await negativeOwnerControl(
			"maintenance",
			fixture,
			pid,
			records,
			"canonical",
			fixture.canonicalPath,
		);
		await negativeOwnerControl("maintenance", fixture, pid, records, "legacy", fixture.legacyPath);
		await runSurfaceBatch("maintenance", fixture, pid);
		assertStateOwners("maintenance", observeOwners(collectTargets(fixture)), pid);
		resumeWorker(worker);
		paused = false;
		const job = await waitForJob(core, ready.socketPath, submitted.jobId, worker);
		assert.equal(job.state, "completed", `maintenance job failed: ${JSON.stringify(job.error)}`);
		validateTrace("maintenance", fixture, pid);
	} finally {
		if (paused) resumeWorker(worker);
		await stopDaemonWorker(worker);
	}
	await stoppedBoundary("maintenance", fixture, core, pid);
}

async function exerciseRestore(fixture, core) {
	const worker = startDaemonWorker(fixture.dataDir, fixture.env);
	const ready = await worker.ready;
	const pid = ready.pid;
	const reason = "Phase 1 T054 restore source";
	rpcResult(
		await rpcRequest(core, ready.socketPath, "POST /v1/backup/create", {
			operationId: "phase1-t054-restore-source",
			reason,
			payloadHash: core.backupPayloadHash(reason),
		}),
		"restore source backup",
	);
	const baseline = readTrace(fixture.tracePath).length;
	let done = false;
	const request = rpcRequest(
		core,
		ready.socketPath,
		"POST /v1/backup/restore",
		{
			operationId: "phase1-t054-restore",
			backupId: "phase1-t054-restore-source",
			payloadHash: core.restorePayloadHash("phase1-t054-restore-source"),
		},
		activeCheckpointDeadlineMs,
	).finally(() => {
		done = true;
	});
	let paused = false;
	try {
		await waitForTrace(
			"restore staging writer",
			worker,
			fixture.tracePath,
			(event, index) =>
				index >= baseline &&
				event.owner === "writer_actor" &&
				basename(event.dbPath).startsWith("restore-"),
		);
		assert.equal(done, false, "restore completed before the active checkpoint");
		await pauseWorker(worker);
		paused = true;
		const records = collectTargets(fixture);
		assertStateOwners("restore", observeOwners(records), pid);
		await negativeOwnerControl("restore", fixture, pid, records, "canonical", fixture.canonicalPath);
		await negativeOwnerControl("restore", fixture, pid, records, "legacy", fixture.legacyPath);
		await runSurfaceBatch("restore", fixture, pid);
		assertStateOwners("restore", observeOwners(collectTargets(fixture)), pid);
		resumeWorker(worker);
		paused = false;
		rpcResult(await request, "restore");
		await waitUntil("restore daemon shutdown", () => core.readDaemonHealth(fixture.dataDir).status === "not_running");
		validateTrace("restore", fixture, pid);
	} finally {
		if (paused) resumeWorker(worker);
		await stopDaemonWorker(worker);
	}
	assert.notEqual(
		currentCanonicalPath(core, fixture.layout),
		fixture.canonicalPath,
		"restore did not switch the canonical pointer",
	);
	await stoppedBoundary("restore", fixture, core, pid);
}

function syntheticNegativeSelfTest() {
	const daemonPid = Math.max(process.pid + 10_000, 20_000);
	for (const state of stateNames) {
		const observation = {
			proc: { canonical: [daemonPid, daemonPid + 1], legacy: [], auxiliary: [] },
			lsof: { canonical: [daemonPid, daemonPid + 1], legacy: [], auxiliary: [] },
		};
		assert.throws(() => assertStateOwners(state, observation, daemonPid));
	}
}

function verifyPrerequisites() {
	assert.equal(process.platform, "linux", "T054 requires Linux /proc semantics");
	assert.equal(typeof process.getuid, "function", "T054 requires POSIX process ownership");
	for (const path of [corePath, cliPath, hookRuntimePath, mcpPath, viewerPath, rigScript, lsofPath]) {
		assert.ok(existsSync(path), `Required T054 artifact is missing: ${path}`);
	}
}

function setupRig(rigBase) {
	const bootstrapHome = join(dirname(rigBase), "bootstrap-home");
	mkdirSync(bootstrapHome, { recursive: true, mode: 0o700 });
	const result = spawnSync("/bin/bash", [rigScript, "setup"], {
		env: { PATH: safePath, HOME: bootstrapHome, USER: "phase1", RIG_BASE: rigBase },
		encoding: "utf8",
		timeout: 15_000,
	});
	assert.equal(result.status, 0, `0B rig setup failed: ${String(result.stderr).trim()}`);
}

function teardownRig(rigBase) {
	spawnSync("/bin/bash", [rigScript, "teardown"], {
		env: {
			PATH: safePath,
			HOME: join(dirname(rigBase), "bootstrap-home"),
			USER: "phase1",
			RIG_BASE: rigBase,
		},
		encoding: "utf8",
		timeout: 15_000,
	});
}

function killTrackedChildren() {
	for (const child of trackedChildren) {
		if (child.exitCode !== null || child.signalCode !== null) continue;
		try {
			process.kill(child.pid, "SIGCONT");
		} catch {
			// The child may not be stopped.
		}
		try {
			process.kill(-child.pid, "SIGKILL");
		} catch {
			child.kill("SIGKILL");
		}
	}
}

async function daemonWorkerMain(dataDir) {
	const core = await import(pathToFileURL(corePath).href);
	let handle = null;
	let stopping = false;
	const stop = async () => {
		if (stopping) return;
		stopping = true;
		try {
			await handle?.stop();
		} finally {
			process.send?.({ type: "stopped" });
			process.disconnect?.();
			process.exit(0);
		}
	};
	process.on("message", (message) => {
		if (message?.type === "stop") void stop();
	});
	process.on("SIGTERM", () => void stop());
	try {
		handle = await core.startDaemon({ dataDir, rpcDeadlineMs: activeCheckpointDeadlineMs });
		process.send?.({
			type: "ready",
			pid: process.pid,
			socketPath: handle.socketPath,
			dataDir: handle.dataDir,
		});
	} catch (error) {
		process.send?.({ type: "error", message: error instanceof Error ? error.message : String(error) });
		process.exit(1);
	}
}

async function viewerWorkerMain() {
	const { createViewerRpcCall } = await import(pathToFileURL(viewerPath).href);
	try {
		const result = await createViewerRpcCall({ timeoutMs: 750 })("GET /v1/health");
		console.log(JSON.stringify({ ok: true, result }));
	} catch (error) {
		console.log(
			JSON.stringify({
				ok: false,
				code: typeof error?.code === "string" ? error.code : "daemon_unavailable",
			}),
		);
	}
}

async function rogueWorkerMain(path) {
	const db = new Database(path, { readonly: true, fileMustExist: true });
	let stopped = false;
	const stop = () => {
		if (stopped) return;
		stopped = true;
		db.close();
		process.disconnect?.();
		process.exit(0);
	};
	process.on("message", (message) => {
		if (message?.type === "stop") stop();
	});
	process.on("SIGTERM", stop);
	process.send?.({ type: "ready" });
}

async function main() {
	verifyPrerequisites();
	syntheticNegativeSelfTest();
	const root = mkdtempSync(join(tmpdir(), "free-mem-t054-"));
	const rigBase = join(root, "rig");
	let rigReady = false;
	try {
		setupRig(rigBase);
		rigReady = true;
		const core = await import(pathToFileURL(corePath).href);
		const baseDataDir = await initializeBase(core, root, rigBase);
		for (const state of stateNames) {
			const fixture = prepareFixture(core, root, rigBase, baseDataDir, state);
			if (state === "down") await exerciseDown(fixture, core);
			else if (state === "up") await exerciseUp(fixture, core);
			else if (state === "maintenance") await exerciseMaintenance(fixture, core);
			else if (state === "backup") await exerciseBackup(fixture, core);
			else if (state === "migration" || state === "legacy-cutover") {
				await exerciseStartupState(state, fixture, core);
			} else if (state === "restore") await exerciseRestore(fixture, core);
			console.log(`PASS T054 ${state}`);
		}
		console.log("PASS T054 runtime DB-open trace: 7 states x 6 independent surfaces");
	} finally {
		killTrackedChildren();
		if (rigReady) teardownRig(rigBase);
		assert.ok(basename(root).startsWith("free-mem-t054-"), "refusing broad temporary cleanup");
		rmSync(root, { recursive: true, force: true });
	}
}

const mode = process.argv[2];
if (mode === "__daemon") await daemonWorkerMain(resolve(process.argv[3]));
else if (mode === "__viewer") await viewerWorkerMain();
else if (mode === "__rogue") await rogueWorkerMain(resolve(process.argv[3]));
else await main();
