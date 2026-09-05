#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
	chmodSync,
	copyFileSync,
	cpSync,
	existsSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readdirSync,
	readlinkSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { pathToFileURL } from "node:url";

const repoRoot = resolve(import.meta.dirname, "..");
const packageRoot = join(repoRoot, "vendor/codemem");
const scriptPath = resolve(import.meta.filename);
const corePath = join(packageRoot, "packages/core/dist/index.js");
const cliPath = join(packageRoot, "packages/cli/dist/index.js");
const safePath = `${dirname(process.execPath)}:/usr/local/bin:/usr/bin:/bin`;
const maxOutputBytes = 1024 * 1024;
const trackedChildren = new Set();
const require = createRequire(join(packageRoot, "packages/core/package.json"));
const Database = require("better-sqlite3");

function appendOutput(current, chunk, child, label) {
	const next = current + String(chunk);
	if (Buffer.byteLength(next, "utf8") <= maxOutputBytes) return next;
	child.kill("SIGKILL");
	throw new Error(`${label} exceeded the output cap.`);
}

async function waitUntil(label, predicate, timeoutMs = 20_000) {
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
		CODEMEM_PROJECT: "phase1-t057",
		...extra,
	};
}

function startDaemonWorker(dataDir, env) {
	let stdout = "";
	let stderr = "";
	let readySettled = false;
	const child = spawn(process.execPath, [scriptPath, "__daemon", dataDir], {
		cwd: packageRoot,
		env,
		stdio: ["ignore", "pipe", "pipe", "ipc"],
	});
	trackedChildren.add(child);
	child.stdout.on("data", (chunk) => {
		stdout = appendOutput(stdout, chunk, child, "daemon stdout");
	});
	child.stderr.on("data", (chunk) => {
		stderr = appendOutput(stderr, chunk, child, "daemon stderr");
	});
	const exit = new Promise((resolveExit) => {
		child.once("exit", (code, signal) => {
			trackedChildren.delete(child);
			resolveExit({ code, signal });
		});
	});
	const ready = new Promise((resolveReady, rejectReady) => {
		const timeout = setTimeout(() => {
			if (readySettled) return;
			readySettled = true;
			child.kill("SIGKILL");
			rejectReady(new Error("daemon did not become ready within 20 seconds"));
		}, 20_000);
		const finish = (callback, value) => {
			if (readySettled) return;
			readySettled = true;
			clearTimeout(timeout);
			callback(value);
		};
		child.on("message", (message) => {
			if (message?.type === "ready") finish(resolveReady, message);
			else if (message?.type === "error") finish(rejectReady, new Error(message.message));
		});
		child.once("error", (error) => finish(rejectReady, error));
		child.once("exit", (code, signal) => {
			finish(
				rejectReady,
				new Error(
					`daemon exited before ready (code=${code}, signal=${signal}): ${stderr.trim()}`,
				),
			);
		});
	});
	return {
		child,
		ready,
		exit,
		get stderr() {
			return stderr;
		},
	};
}

async function stopWorker(worker) {
	if (worker.child.exitCode !== null || worker.child.signalCode !== null) return;
	try {
		process.kill(worker.child.pid, "SIGCONT");
	} catch {
		// The child may not be stopped.
	}
	worker.child.send({ type: "stop" });
	const outcome = await Promise.race([
		worker.exit.then(() => "exited"),
		sleep(5_000).then(() => "timeout"),
	]);
	if (outcome === "timeout") worker.child.kill("SIGKILL");
	await worker.exit;
}

async function pauseWorker(worker) {
	assert.ok(worker.child.pid > 1 && worker.child.pid !== process.pid);
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

function runProcess(name, args, env, cwd = packageRoot, timeoutMs = 20_000) {
	return new Promise((resolveResult, rejectResult) => {
		let stdout = "";
		let stderr = "";
		let timedOut = false;
		const child = spawn(process.execPath, args, {
			cwd,
			env,
			stdio: ["ignore", "pipe", "pipe"],
		});
		trackedChildren.add(child);
		child.stdout.on("data", (chunk) => {
			stdout = appendOutput(stdout, chunk, child, `${name} stdout`);
		});
		child.stderr.on("data", (chunk) => {
			stderr = appendOutput(stderr, chunk, child, `${name} stderr`);
		});
		child.once("error", rejectResult);
		const timeout = setTimeout(() => {
			timedOut = true;
			child.kill("SIGKILL");
		}, timeoutMs);
		child.once("close", (code, signal) => {
			clearTimeout(timeout);
			trackedChildren.delete(child);
			resolveResult({ name, code, signal, stdout, stderr, timedOut });
		});
	});
}

async function runCliJson(args, env) {
	const result = await runProcess(`codemem ${args.join(" ")}`, [cliPath, ...args], env);
	assert.equal(result.timedOut, false, `${result.name} timed out`);
	assert.equal(result.signal, null, `${result.name} was killed by ${result.signal}`);
	assert.equal(result.code, 0, `${result.name} failed: ${result.stderr.trim()}`);
	assert.ok(result.stdout.trim(), `${result.name} produced no JSON`);
	return JSON.parse(result.stdout);
}

function rpcRequest(core, method, body) {
	return {
		id: randomUUID(),
		method,
		adapter_version: "phase1-t057",
		native_cli_version: "phase1-t057",
		normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
		local_api_version: core.LOCAL_API_VERSION,
		capability_hash: core.RPC_CAPABILITY_HASH,
		body,
	};
}

async function rpc(core, socketPath, method, body) {
	const response = await core.callDaemonRpc(socketPath, rpcRequest(core, method, body), {
		timeoutMs: 15_000,
	});
	assert.ok(!("error" in response), `${method} failed: ${JSON.stringify(response)}`);
	return response.result;
}

function currentDatabasePath(core, layout) {
	const pointer = core.readCurrentDatabasePointer(layout);
	assert.ok(pointer, `canonical pointer missing: ${layout.dataDir}`);
	return join(layout.dbDir, pointer);
}

function writePrivateJson(path, value) {
	writeFileSync(path, `${JSON.stringify(value)}\n`, { mode: 0o600, flush: true });
	chmodSync(path, 0o600);
}

function copyPrivateFile(source, destination) {
	copyFileSync(source, destination);
	chmodSync(destination, 0o600);
}

function assertProductionBackupValid(core, dataDir, backupId, expectedManifestHash) {
	const check = core.verifyCanonicalBackup({ dataDir, backupId });
	assert.equal(check.valid, true, check.diagnostics.join("; "));
	assert.equal(check.manifestHash, expectedManifestHash);
}

function assertCurrentMatchesManifest(core, dataDir, sourceSidecar, checkId, reason) {
	const layout = core.resolveStorageLayout(dataDir);
	const artifact = join(layout.backupsDir, `${checkId}.sqlite`);
	const sidecarPath = join(layout.backupsDir, `${checkId}.json`);
	copyPrivateFile(currentDatabasePath(core, layout), artifact);
	const manifest = {
		...sourceSidecar.manifest,
		operation_id: checkId,
		reason,
		payload_hash: core.backupPayloadHash(reason),
		artifact_sha256: core.sha256File(artifact),
	};
	const sidecar = {
		...sourceSidecar,
		manifest,
		manifest_hash: core.hashMutationPayload(manifest),
	};
	writePrivateJson(sidecarPath, sidecar);
	assertProductionBackupValid(core, dataDir, checkId, sidecar.manifest_hash);
}

async function seedMemory(core, socketPath, idempotencyKey, title, body) {
	const result = await rpc(core, socketPath, "POST /v1/memories/record", {
		idempotencyKey,
		kind: "decision",
		title,
		body,
	});
	assert.equal(typeof result.memoryId, "number");
}

async function freshRestoreGate(core, root) {
	const sourceDataDir = join(root, "fresh-source-data");
	const sourceEnv = runtimeEnv(sourceDataDir, join(root, "fresh-source-env"));
	let sourceWorker = startDaemonWorker(sourceDataDir, sourceEnv);
	let sourceReady = await sourceWorker.ready;
	await seedMemory(
		core,
		sourceReady.socketPath,
		"t057-memory-one",
		"T057 FTS restore one",
		"t057ftsneedle first canonical row",
	);
	await seedMemory(
		core,
		sourceReady.socketPath,
		"t057-memory-two",
		"T057 FTS restore two",
		"t057ftsneedle second canonical row",
	);
	await stopWorker(sourceWorker);

	const sourceLayout = core.resolveStorageLayout(sourceDataDir);
	const sourceCurrent = currentDatabasePath(core, sourceLayout);
	const sourceDb = new Database(sourceCurrent);
	try {
		sourceDb.pragma("journal_mode = DELETE");
		sourceDb.exec("INSERT INTO memory_fts(memory_fts) VALUES('delete-all')");
		assert.equal(
			sourceDb
				.prepare("SELECT COUNT(*) FROM memory_fts WHERE memory_fts MATCH 't057ftsneedle'")
				.pluck()
				.get(),
			0,
		);
	} finally {
		sourceDb.close();
	}

	sourceWorker = startDaemonWorker(sourceDataDir, sourceEnv);
	sourceReady = await sourceWorker.ready;
	const created = await runCliJson(
		["backup", "create", "--reason", "Phase 1 T057 fresh restore", "--json"],
		sourceEnv,
	);
	assert.equal(typeof created.backupId, "string");
	assert.match(created.artifactSha256, /^[a-f0-9]{64}$/);
	assert.match(created.manifestHash, /^[a-f0-9]{64}$/);
	const backupId = created.backupId;
	const verified = await runCliJson(["backup", "verify", backupId, "--json"], sourceEnv);
	assert.equal(verified.valid, true);
	assert.equal(verified.manifestHash, created.manifestHash);

	const sourceArtifact = join(sourceLayout.backupsDir, `${backupId}.sqlite`);
	const sourceSidecarPath = join(sourceLayout.backupsDir, `${backupId}.json`);
	const sourceSidecar = JSON.parse(readFileSync(sourceSidecarPath, "utf8"));
	const backupDb = new Database(sourceArtifact, { readonly: true, fileMustExist: true });
	try {
		assert.equal(
			backupDb
				.prepare("SELECT COUNT(*) FROM memory_fts WHERE memory_fts MATCH 't057ftsneedle'")
				.pluck()
				.get(),
			0,
			"backup fixture unexpectedly contained a populated FTS index",
		);
	} finally {
		backupDb.close();
	}
	assert.equal(sourceSidecar.manifest_hash, core.hashMutationPayload(sourceSidecar.manifest));
	assert.equal(sourceSidecar.manifest.artifact_sha256, core.sha256File(sourceArtifact));
	assert.equal(sourceSidecar.manifest_hash, created.manifestHash);
	assert.ok(sourceSidecar.manifest.canonical_tables.length > 0);
	assert.ok(
		sourceSidecar.manifest.canonical_tables.every(
			(table) =>
				Number.isInteger(table.row_count) &&
				table.row_count >= 0 &&
				/^[a-f0-9]{64}$/.test(table.checksum_sha256),
		),
	);
	assert.ok(
		sourceSidecar.manifest.canonical_tables.some(
			(table) => table.name === "memory_items" && table.row_count === 2,
		),
		"backup manifest did not capture both canonical memories",
	);
	assertProductionBackupValid(core, sourceDataDir, backupId, sourceSidecar.manifest_hash);

	await seedMemory(
		core,
		sourceReady.socketPath,
		"t057-memory-after-backup",
		"T057 source-only row",
		"this row must not cross the backup boundary",
	);
	await stopWorker(sourceWorker);
	const sourcePointerBeforeRestore = core.readCurrentDatabasePointer(sourceLayout);
	const sourceHashBeforeRestore = core.sha256File(currentDatabasePath(core, sourceLayout));

	const freshDataDir = join(root, "fresh-target-data");
	assert.equal(existsSync(freshDataDir), false, "fresh target was already used");
	const freshLayout = core.resolveStorageLayout(freshDataDir);
	core.ensureStorageLayout(freshLayout);
	assert.equal(core.readCurrentDatabasePointer(freshLayout), null);
	const freshArtifact = join(freshLayout.backupsDir, `${backupId}.sqlite`);
	const freshSidecarPath = join(freshLayout.backupsDir, `${backupId}.json`);
	copyPrivateFile(sourceArtifact, freshArtifact);
	copyPrivateFile(sourceSidecarPath, freshSidecarPath);
	const freshSidecar = JSON.parse(readFileSync(freshSidecarPath, "utf8"));
	const unavailableVector = freshSidecar.manifest.sqlite_vec ?? {
		version: "unavailable-for-t057",
		platform: `${process.platform}-${process.arch}`,
	};
	freshSidecar.manifest.sqlite_vec = {
		...unavailableVector,
		artifact_sha256: "0".repeat(64),
	};
	freshSidecar.manifest_hash = core.hashMutationPayload(freshSidecar.manifest);
	writePrivateJson(freshSidecarPath, freshSidecar);
	assert.equal(freshSidecar.manifest.artifact_sha256, core.sha256File(freshArtifact));
	assertProductionBackupValid(core, freshDataDir, backupId, freshSidecar.manifest_hash);

	const freshEnv = runtimeEnv(freshDataDir, join(root, "fresh-target-env"));
	let freshWorker = startDaemonWorker(freshDataDir, freshEnv);
	let freshReady = await freshWorker.ready;
	const bootstrapPointer = core.readCurrentDatabasePointer(freshLayout);
	const freshVerified = await runCliJson(["backup", "verify", backupId, "--json"], freshEnv);
	assert.equal(freshVerified.valid, true);
	assert.equal(freshVerified.manifestHash, freshSidecar.manifest_hash);
	const restored = await runCliJson(["backup", "restore", backupId, "--json"], freshEnv);
	assert.equal(restored.backupId, backupId);
	assert.equal(restored.restartRequired, true);
	await waitUntil(
		"restore-triggered daemon shutdown",
		() => core.readDaemonHealth(freshDataDir).status === "not_running",
	);
	await stopWorker(freshWorker);
	const restoredPointer = core.readCurrentDatabasePointer(freshLayout);
	assert.ok(restoredPointer && restoredPointer !== bootstrapPointer);
	assert.equal(core.readCurrentDatabasePointer(sourceLayout), sourcePointerBeforeRestore);
	assert.equal(core.sha256File(currentDatabasePath(core, sourceLayout)), sourceHashBeforeRestore);

	const restoredPath = currentDatabasePath(core, freshLayout);
	assert.equal(restored.pointer, restoredPointer);
	assert.equal(restored.artifactSha256, core.sha256File(restoredPath));
	assert.equal(restored.manifestHash, freshSidecar.manifest_hash);
	const restoredDb = new Database(restoredPath, { readonly: true, fileMustExist: true });
	try {
		assert.equal(
			restoredDb
				.prepare("SELECT COUNT(*) FROM memory_fts WHERE memory_fts MATCH 't057ftsneedle'")
				.pluck()
				.get(),
			2,
			"restore did not rebuild FTS",
		);
		assert.deepEqual(
			restoredDb.prepare("SELECT title FROM memory_items ORDER BY id").pluck().all(),
			["T057 FTS restore one", "T057 FTS restore two"],
		);
	} finally {
		restoredDb.close();
	}

	assertCurrentMatchesManifest(
		core,
		freshDataDir,
		freshSidecar,
		"t057-current-artifact",
		"Phase 1 T057 restored current verification",
	);

	freshWorker = startDaemonWorker(freshDataDir, freshEnv);
	freshReady = await freshWorker.ready;
	const search = await rpc(core, freshReady.socketPath, "POST /v1/search", {
		requestId: "t057-fts-search",
		mode: "search",
		query: "t057ftsneedle",
		limit: 10,
	});
	assert.equal(search.items.length, 2, "FTS-only RPC search did not recover both memories");
	await stopWorker(freshWorker);

	const sourceDbAfter = new Database(currentDatabasePath(core, sourceLayout), {
		readonly: true,
		fileMustExist: true,
	});
	try {
		assert.deepEqual(
			sourceDbAfter.prepare("SELECT title FROM memory_items ORDER BY id").pluck().all(),
			["T057 FTS restore one", "T057 FTS restore two", "T057 source-only row"],
		);
	} finally {
		sourceDbAfter.close();
	}
	console.log("PASS T057 fresh-dir restore: built CLI/RPC, manifest/hash, canonical rows, FTS-only");
}

async function initializeJournalBase(core, root) {
	const dataDir = join(root, "journal-base-data");
	const env = runtimeEnv(dataDir, join(root, "journal-base-env"));
	const worker = startDaemonWorker(dataDir, env);
	await worker.ready;
	await stopWorker(worker);
	return dataDir;
}

async function expectDaemonStartFailure(worker, label, pattern) {
	let error = null;
	try {
		await worker.ready;
	} catch (caught) {
		error = caught;
	}
	if (error === null) {
		await stopWorker(worker);
		assert.fail(`${label} unexpectedly started`);
	}
	const exit = await worker.exit;
	assert.notEqual(exit.code, 0, `${label} exited successfully`);
	assert.equal(exit.signal, null, `${label} was killed by ${exit.signal}`);
	assert.match(`${error instanceof Error ? error.message : String(error)}\n${worker.stderr}`, pattern);
}

async function journalFailClosedGate(core, root) {
	const baseDataDir = await initializeJournalBase(core, root);
	const cases = [
		{
			name: "empty",
			journal: () => "",
			pattern: /unreadable|malformed/i,
		},
		{
			name: "partial",
			journal: () => '{"version":1',
			pattern: /unreadable|malformed/i,
		},
		{
			name: "invalid-state",
			journal: (pointer, hash) =>
				JSON.stringify({
					version: 1,
					operationId: "t057-invalid-state",
					state: "unknown",
					oldPointer: null,
					newPointer: pointer,
					artifactSha256: hash,
				}),
			pattern: /invalid state/i,
		},
		{
			name: "artifact-hash-mismatch",
			journal: (pointer) =>
				JSON.stringify({
					version: 1,
					operationId: "t057-hash-mismatch",
					state: "committed",
					oldPointer: null,
					newPointer: pointer,
					artifactSha256: "0".repeat(64),
				}),
			pattern: /hash mismatch/i,
		},
		{
			name: "ambiguous-pointer",
			journal: (_pointer, hash) =>
				JSON.stringify({
					version: 1,
					operationId: "t057-ambiguous-pointer",
					state: "prepared",
					oldPointer: "versions/t057-old.sqlite",
					newPointer: "versions/t057-new.sqlite",
					artifactSha256: hash,
				}),
			pattern: /does not identify the current database pointer/i,
		},
	];
	for (const item of cases) {
		const dataDir = join(root, `journal-${item.name}-data`);
		cpSync(baseDataDir, dataDir, {
			recursive: true,
			preserveTimestamps: true,
			verbatimSymlinks: true,
		});
		const layout = core.resolveStorageLayout(dataDir);
		const pointer = core.readCurrentDatabasePointer(layout);
		assert.ok(pointer);
		const currentPath = join(layout.dbDir, pointer);
		const beforeHash = core.sha256File(currentPath);
		const journalContents = item.journal(pointer, beforeHash);
		writeFileSync(layout.journalPath, journalContents, { mode: 0o600, flush: true });
		const worker = startDaemonWorker(
			dataDir,
			runtimeEnv(dataDir, join(root, `journal-${item.name}-env`)),
		);
		await expectDaemonStartFailure(worker, item.name, item.pattern);
		assert.equal(core.readCurrentDatabasePointer(layout), pointer, `${item.name} changed pointer`);
		assert.equal(core.sha256File(currentPath), beforeHash, `${item.name} changed current artifact`);
		assert.equal(existsSync(layout.journalPath), true, `${item.name} removed its journal`);
		assert.equal(readFileSync(layout.journalPath, "utf8"), journalContents);
	}
	console.log("PASS T057 storage journal corruption/ambiguity fails closed in a real daemon process");
}

function createLegacyFixture(core, root, baselineArtifact, label) {
	const fixtureRoot = join(root, `legacy-${label}`);
	const dataDir = join(fixtureRoot, "data");
	const layout = core.resolveStorageLayout(dataDir);
	core.ensureStorageLayout(layout);
	const legacyPath = join(dataDir, "mem.sqlite");
	copyPrivateFile(baselineArtifact, legacyPath);
	const db = new Database(legacyPath);
	try {
		db.pragma("journal_mode = WAL");
		db.exec(`
			CREATE TABLE t057_legacy_probe (value TEXT PRIMARY KEY);
			INSERT INTO t057_legacy_probe(value) VALUES ('last-committed-before-cutover');
			CREATE TABLE t057_legacy_payload (payload BLOB NOT NULL);
			INSERT INTO t057_legacy_payload(payload) VALUES (zeroblob(${128 * 1024 * 1024}));
		`);
		db.pragma("wal_checkpoint(TRUNCATE)");
	} finally {
		db.close();
	}
	const managedPath = join(fixtureRoot, "thin-client.sh");
	writeFileSync(managedPath, "#!/bin/sh\nexec codemem codex-hook-ingest\n", { mode: 0o600 });
	core.writeInstallManifest(layout.installManifestPath, {
		version: 1,
		blocks: [],
		targets: [core.captureManagedTarget("thin-client", managedPath)],
	});
	return { fixtureRoot, dataDir, layout, legacyPath };
}

function startLegacyIdle(path, env) {
	let stderr = "";
	let settled = false;
	const child = spawn(
		process.execPath,
		[scriptPath, "__legacy_idle_rw", path, "codemem", "serve"],
		{ cwd: packageRoot, env, stdio: ["ignore", "ignore", "pipe", "ipc"] },
	);
	trackedChildren.add(child);
	child.stderr.on("data", (chunk) => {
		stderr = appendOutput(stderr, chunk, child, "legacy idle stderr");
	});
	const exit = new Promise((resolveExit) => {
		child.once("exit", (code, signal) => {
			trackedChildren.delete(child);
			resolveExit({ code, signal });
		});
	});
	const ready = new Promise((resolveReady, rejectReady) => {
		const timeout = setTimeout(() => {
			if (settled) return;
			settled = true;
			child.kill("SIGKILL");
			rejectReady(new Error("legacy idle writer did not become ready"));
		}, 10_000);
		child.once("message", (message) => {
			if (settled) return;
			settled = true;
			clearTimeout(timeout);
			if (message?.type === "ready") resolveReady(message);
			else rejectReady(new Error(message?.message ?? "legacy idle writer failed"));
		});
		child.once("error", rejectReady);
		child.once("exit", (code) => {
			if (settled) return;
			settled = true;
			clearTimeout(timeout);
			rejectReady(new Error(`legacy idle writer exited early (${code}): ${stderr}`));
		});
	});
	return { child, ready, exit };
}

async function stopLegacyIdle(holder) {
	if (holder.child.exitCode !== null || holder.child.signalCode !== null) return;
	holder.child.send({ type: "stop" });
	const outcome = await Promise.race([
		holder.exit.then(() => "exited"),
		sleep(2_000).then(() => "timeout"),
	]);
	if (outcome === "timeout") holder.child.kill("SIGKILL");
	await holder.exit;
}

async function legacyProcessGate(core, root) {
	const baselineDataDir = join(root, "legacy-baseline-data");
	const baselineEnv = runtimeEnv(baselineDataDir, join(root, "legacy-baseline-env"));
	const baselineWorker = startDaemonWorker(baselineDataDir, baselineEnv);
	await baselineWorker.ready;
	await stopWorker(baselineWorker);
	const baselineLayout = core.resolveStorageLayout(baselineDataDir);
	const baselineArtifact = currentDatabasePath(core, baselineLayout);

	const idle = createLegacyFixture(core, root, baselineArtifact, "idle");
	const idleHash = core.sha256File(idle.legacyPath);
	const idleEnv = runtimeEnv(idle.dataDir, join(root, "legacy-idle-env"));
	const holder = startLegacyIdle(idle.legacyPath, idleEnv);
	await holder.ready;
	const blockedWorker = startDaemonWorker(idle.dataDir, idleEnv);
	await expectDaemonStartFailure(blockedWorker, "legacy idle RW handle", /owner did not stop|open handle/i);
	assert.equal(holder.child.exitCode, null, "daemon unexpectedly killed the uncooperative holder");
	assert.equal(core.readCurrentDatabasePointer(idle.layout), null);
	assert.equal(existsSync(idle.layout.journalPath), false);
	assert.equal(core.sha256File(idle.legacyPath), idleHash);
	assert.equal(lstatSync(idle.legacyPath).isFile(), true);
	await stopLegacyIdle(holder);

	const active = createLegacyFixture(core, root, baselineArtifact, "backup-writer");
	const tracePath = join(active.fixtureRoot, "db-open-trace.jsonl");
	writeFileSync(tracePath, "", { mode: 0o600 });
	const activeEnv = runtimeEnv(active.dataDir, join(root, "legacy-active-env"), {
		CODEMEM_DB_OPEN_TRACE: tracePath,
	});
	const cutoverWorker = startDaemonWorker(active.dataDir, activeEnv);
	let paused = false;
	try {
		await waitUntil(
			"legacy online backup temp",
			() =>
				readdirSync(active.layout.backupsDir).some((name) => name.endsWith(".tmp")) ||
				cutoverWorker.child.exitCode !== null,
			30_000,
		);
		assert.equal(cutoverWorker.child.exitCode, null, cutoverWorker.stderr);
		await pauseWorker(cutoverWorker);
		paused = true;
		const writerAttempt = await runProcess(
			"legacy writer during backup",
			[scriptPath, "__legacy_write", active.legacyPath, "during-backup"],
			activeEnv,
		);
		assert.equal(writerAttempt.timedOut, false);
		assert.equal(writerAttempt.signal, null);
		assert.notEqual(writerAttempt.code, 0, "legacy writer escaped the EXCLUSIVE handoff");
		assert.match(writerAttempt.stderr, /busy|locked/i);
		resumeWorker(cutoverWorker);
		paused = false;
		await cutoverWorker.ready;
	} finally {
		if (paused) resumeWorker(cutoverWorker);
		await stopWorker(cutoverWorker);
	}

	assert.equal(lstatSync(active.legacyPath).isSymbolicLink(), true);
	assert.equal(statSync(readlinkSync(active.legacyPath)).isDirectory(), true);
	assert.equal(existsSync(`${active.legacyPath}-wal`), false);
	assert.equal(existsSync(`${active.legacyPath}-shm`), false);
	const postCommitAttempt = await runProcess(
		"old binary post-commit restart",
		[scriptPath, "__legacy_write", active.legacyPath, "post-commit"],
		activeEnv,
	);
	assert.equal(postCommitAttempt.timedOut, false);
	assert.equal(postCommitAttempt.signal, null);
	assert.notEqual(postCommitAttempt.code, 0, "old binary created a split-brain database");
	assert.equal(lstatSync(active.legacyPath).isSymbolicLink(), true);
	assert.equal(existsSync(`${active.legacyPath}-wal`), false);
	assert.equal(existsSync(`${active.legacyPath}-shm`), false);

	const pointer = core.readCurrentDatabasePointer(active.layout);
	assert.ok(pointer);
	assert.deepEqual(
		readdirSync(active.layout.versionsDir).filter((name) => name.endsWith(".sqlite")),
		[basename(pointer)],
	);
	const canonical = new Database(join(active.layout.dbDir, pointer), {
		readonly: true,
		fileMustExist: true,
	});
	try {
		assert.equal(
			canonical.prepare("SELECT value FROM t057_legacy_probe").pluck().get(),
			"last-committed-before-cutover",
		);
		assert.equal(
			canonical
				.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='t057_split_brain'")
				.get(),
			undefined,
		);
	} finally {
		canonical.close();
	}
	const backupId = basename(pointer, ".sqlite");
	const backupSidecar = JSON.parse(
		readFileSync(join(active.layout.backupsDir, `${backupId}.json`), "utf8"),
	);
	assertProductionBackupValid(core, active.dataDir, backupId, backupSidecar.manifest_hash);
	assertCurrentMatchesManifest(
		core,
		active.dataDir,
		backupSidecar,
		"t057-legacy-current",
		"Phase 1 T057 legacy current verification",
	);
	console.log("PASS T057 legacy process fencing: idle RW, backup writer, old binary restart");
}

function verifyPrerequisites() {
	assert.equal(process.platform, "linux", "T057 requires Linux /proc, signals, and Unix sockets");
	assert.equal(typeof process.getuid, "function", "T057 requires POSIX process ownership");
	for (const path of [corePath, cliPath]) {
		assert.ok(existsSync(path), `required T057 built artifact missing: ${path}`);
	}
}

function killTrackedChildren() {
	for (const child of trackedChildren) {
		try {
			process.kill(child.pid, "SIGCONT");
		} catch {
			// The child may not be stopped.
		}
		try {
			child.kill("SIGKILL");
		} catch {
			// Best effort after a failed assertion.
		}
	}
}

async function daemonChild(dataDir) {
	const core = await import(pathToFileURL(corePath).href);
	let handle;
	try {
		handle = await core.startDaemon({ dataDir, rpcDeadlineMs: 15_000 });
		process.send?.({
			type: "ready",
			pid: process.pid,
			socketPath: handle.socketPath,
		});
	} catch (error) {
		process.send?.({ type: "error", message: error instanceof Error ? error.message : String(error) });
		process.disconnect?.();
		process.exit(1);
	}
	process.on("message", async (message) => {
		if (message?.type !== "stop") return;
		try {
			await handle.stop();
		} finally {
			process.disconnect?.();
			process.exit(0);
		}
	});
}

function legacyWriteChild(path, value) {
	try {
		const db = new Database(path, { fileMustExist: true });
		try {
			db.pragma("busy_timeout = 0");
			db.exec("CREATE TABLE IF NOT EXISTS t057_split_brain (value TEXT NOT NULL)");
			db.prepare("INSERT INTO t057_split_brain(value) VALUES (?)").run(value);
		} finally {
			db.close();
		}
	} catch (error) {
		console.error(error instanceof Error ? error.message : String(error));
		process.exit(1);
	}
}

function legacyIdleChild(path) {
	let db;
	try {
		db = new Database(path, { fileMustExist: true });
		db.pragma("busy_timeout = 0");
		process.send?.({ type: "ready" });
	} catch (error) {
		process.send?.({ type: "error", message: error instanceof Error ? error.message : String(error) });
		process.exit(1);
	}
	process.on("SIGTERM", () => {
		// Simulate an old process that cannot complete the requested clean stop.
	});
	process.on("message", (message) => {
		if (message?.type !== "stop") return;
		db.close();
		process.disconnect?.();
		process.exit(0);
	});
}

async function main() {
	verifyPrerequisites();
	const core = await import(pathToFileURL(corePath).href);
	const root = mkdtempSync(join(tmpdir(), "free-mem-t057-"));
	try {
		await freshRestoreGate(core, root);
		await journalFailClosedGate(core, root);
		await legacyProcessGate(core, root);
		console.log("PASS T057 backup restore real-process smoke");
	} finally {
		killTrackedChildren();
		assert.ok(basename(root).startsWith("free-mem-t057-"), "refusing broad temporary cleanup");
		rmSync(root, { recursive: true, force: true });
	}
}

const mode = process.argv[2];
if (mode === "__daemon") await daemonChild(resolve(process.argv[3]));
else if (mode === "__legacy_write") legacyWriteChild(resolve(process.argv[3]), process.argv[4]);
else if (mode === "__legacy_idle_rw") legacyIdleChild(resolve(process.argv[3]));
else await main();
