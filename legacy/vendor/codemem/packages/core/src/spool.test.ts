import { spawn } from "node:child_process";
import {
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	realpathSync,
	statSync,
	truncateSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { startDaemon } from "./daemon-lifecycle.js";
import {
	callDaemonRpc,
	LOCAL_API_VERSION,
	NORMALIZED_SCHEMA_VERSION,
	RPC_CAPABILITY_HASH,
} from "./daemon-rpc.js";
import { parseAgentMemoryToml } from "./redaction-pipeline.js";
import {
	acquireSpoolLock,
	drainLegacySpool,
	quarantineSpoolEntry,
	readSpoolStatus,
	resolveSpoolLayout,
	SPOOL_FILE_MAX_BYTES,
	SPOOL_NORMAL_QUOTA_BYTES,
	SPOOL_QUARANTINE_QUOTA_BYTES,
	SPOOL_RESERVED_MIN_EVENTS,
	SPOOL_RESERVED_QUOTA_BYTES,
	spoolMutation,
	validateSpoolRedaction,
} from "./spool.js";
import { resolveStorageLayout } from "./storage-layout.js";
import { ReadOnlyActor } from "./writer-actor.js";

const fsFault = vi.hoisted(() => ({
	renameDiskFull: false,
	tmpWrite: false,
	flushWrite: false,
	fsyncSuffix: "",
	lstatMissingOnce: "",
	lstatFaultCode: "ENOENT",
	lstatMissingArmOnRead: "",
	readyUnlink: false,
}));

vi.mock("node:fs", async (importOriginal) => {
	const actual = await importOriginal<typeof import("node:fs")>();
	const ioError = (message: string): NodeJS.ErrnoException => {
		const error = new Error(message) as NodeJS.ErrnoException;
		error.code = "EIO";
		return error;
	};
	return {
		...actual,
		writeFileSync(...args: Parameters<typeof actual.writeFileSync>): void {
			if (fsFault.tmpWrite && String(args[0]).endsWith(".json.tmp")) {
				throw ioError("synthetic spool temp write failure");
			}
			if (fsFault.flushWrite && String(args[0]).endsWith(".json.tmp")) {
				Reflect.apply(actual.writeFileSync, actual, [
					args[0],
					args[1],
					{ ...(args[2] as object), flush: false },
				]);
				throw ioError("synthetic spool flush failure");
			}
			actual.writeFileSync(...args);
		},
		fsyncSync(fd: number): void {
			const target = actual.readlinkSync(`/proc/self/fd/${fd}`);
			if (fsFault.fsyncSuffix && target.endsWith(fsFault.fsyncSuffix)) {
				throw ioError("synthetic spool fsync failure");
			}
			actual.fsyncSync(fd);
		},
		readFileSync(...args: Parameters<typeof actual.readFileSync>) {
			// While this holds a path fragment, lstatMissingOnce stays disarmed. removeStaleLock
			// probes the lock owner's liveness through that path between its two stats, so
			// arming there lands the fault on the second stat rather than the guarded first one.
			if (
				fsFault.lstatMissingArmOnRead &&
				String(args[0]).includes(fsFault.lstatMissingArmOnRead)
			) {
				fsFault.lstatMissingArmOnRead = "";
			}
			return actual.readFileSync(...args);
		},
		lstatSync(...args: Parameters<typeof actual.lstatSync>) {
			if (!fsFault.lstatMissingArmOnRead && fsFault.lstatMissingOnce === String(args[0])) {
				fsFault.lstatMissingOnce = "";
				const error = new Error("synthetic spool lock stat failure") as NodeJS.ErrnoException;
				error.code = fsFault.lstatFaultCode;
				throw error;
			}
			return actual.lstatSync(...args);
		},
		renameSync(source: string, destination: string): void {
			if (fsFault.renameDiskFull && destination.includes("/control/spool/ready/")) {
				const error = new Error("no space left on device") as NodeJS.ErrnoException;
				error.code = "ENOSPC";
				throw error;
			}
			actual.renameSync(source, destination);
		},
		unlinkSync(path: string): void {
			if (fsFault.readyUnlink && path.includes("/control/spool/ready/")) {
				throw ioError("synthetic durable spool delete failure");
			}
			actual.unlinkSync(path);
		},
	};
});

const roots: string[] = [];

afterEach(async () => {
	fsFault.renameDiskFull = false;
	fsFault.tmpWrite = false;
	fsFault.flushWrite = false;
	fsFault.fsyncSuffix = "";
	fsFault.lstatMissingOnce = "";
	fsFault.lstatFaultCode = "ENOENT";
	fsFault.lstatMissingArmOnRead = "";
	fsFault.readyUnlink = false;
	await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function tempDataDir(): Promise<string> {
	const root = await mkdtemp(join(tmpdir(), "codemem-spool-"));
	roots.push(root);
	return join(root, "data");
}

function eventBody(
	idempotencyKey: string,
	kind = "tool_completed",
	payload: Record<string, unknown> = { text: "ok" },
): Record<string, unknown> {
	return {
		idempotencyKey,
		event: {
			schemaVersion: 1,
			eventId: `event-${idempotencyKey}`,
			idempotencyKey,
			agent: "codex",
			nativeSessionId: "session-1",
			projectKey: "project-1",
			workspaceKey: "workspace-1",
			cwd: "/tmp/project",
			kind,
			occurredAt: "2026-08-14T00:00:00.000Z",
			payload,
			sourceHash: "a".repeat(64),
			sensitivity: "normal",
		},
	};
}

function expectExactlyOneEvent(dataDir: string, key: string): void {
	const reader = ReadOnlyActor.open(realpathSync(resolveStorageLayout(dataDir).currentPointerPath));
	try {
		expect(
			reader
				.prepare(
					"SELECT COUNT(*) AS n FROM mutation_receipts WHERE method = 'POST /v1/events' AND idempotency_key = ?",
				)
				.get(key),
		).toEqual({ n: 1 });
		expect(
			reader.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE event_id = ?").get(`event-${key}`),
		).toEqual({ n: 1 });
	} finally {
		reader.close();
	}
}

function writeSparse(path: string, bytes: number): void {
	mkdirSync(dirname(path), { recursive: true });
	writeFileSync(path, "", { mode: 0o600 });
	truncateSync(path, bytes);
}

type WriterResult = { status: string; reason?: string };

function runWriter(dataDir: string, key: string): Promise<WriterResult> {
	const moduleUrl = pathToFileURL(fileURLToPath(new URL("./spool.ts", import.meta.url))).href;
	const workerUrl = pathToFileURL(
		fileURLToPath(new URL("./redaction-worker.ts", import.meta.url)),
	).href;
	const script = `
		import { spoolMutation } from ${JSON.stringify(moduleUrl)};
		import { warmRedactionWorker } from ${JSON.stringify(workerUrl)};
		if (!warmRedactionWorker()) throw new Error("redaction worker did not start");
		const key = ${JSON.stringify(key)};
		const body = ${JSON.stringify(eventBody(key))};
		const result = spoolMutation(
			{ method: "POST /v1/events", idempotencyKey: key, body },
			{ dataDir: ${JSON.stringify(dataDir)}, lockDeadlineMs: 250, onWarning: () => {} },
		);
		process.stdout.write(JSON.stringify({ status: result.status, reason: result.reason }));
	`;
	return new Promise((resolve, reject) => {
		const child = spawn(
			process.execPath,
			["--import", "tsx", "--input-type=module", "-e", script],
			{
				stdio: ["ignore", "pipe", "pipe"],
			},
		);
		let stdout = "";
		let stderr = "";
		child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
			stdout += chunk;
		});
		child.stderr.setEncoding("utf8").on("data", (chunk: string) => {
			stderr += chunk;
		});
		child.on("error", reject);
		child.on("close", (code) => {
			if (code !== 0) reject(new Error(`writer exited ${code}: ${stderr}`));
			else resolve(JSON.parse(stdout) as WriterResult);
		});
	});
}

describe("phase 1 spool contract", () => {
	it("P1-T039-01-quota-warning-reserve", async () => {
		const dataDir = await tempDataDir();
		const layout = resolveSpoolLayout(dataDir);
		const warnings: string[] = [];
		const secret = `ghp_${"A".repeat(36)}`;
		expect(SPOOL_RESERVED_QUOTA_BYTES / SPOOL_FILE_MAX_BYTES).toBeGreaterThanOrEqual(
			SPOOL_RESERVED_MIN_EVENTS,
		);

		writeSparse(
			join(layout.readyDir, "normal-existing.json"),
			Math.ceil(SPOOL_NORMAL_QUOTA_BYTES * 0.8),
		);
		const warningResult = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "warning",
				body: eventBody("warning", "tool_completed", {
					text: `keep ${secret} <private>hidden</private> <local-only>device</local-only>`,
				}),
			},
			{ dataDir, onWarning: (message) => warnings.push(message) },
		);
		expect(warningResult.status).toBe("queued");
		expect(warnings.some((message) => message.includes("80%"))).toBe(true);
		const persisted = readFileSync(warningResult.path as string, "utf8");
		expect(persisted).not.toContain(secret);
		expect(persisted).not.toContain("hidden");
		expect(persisted).toContain("[REDACTED:");
		expect(JSON.parse(persisted)).toMatchObject({
			redaction: {
				sensitivity: "secret",
				redaction_degraded: false,
				private_content_omitted: true,
				local_only: true,
			},
		});
		const daemon = await startDaemon({ dataDir });
		try {
			const health = await callDaemonRpc(daemon.socketPath, {
				id: "spool-health",
				method: "GET /v1/health",
				adapter_version: "test",
				native_cli_version: "test",
				normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
				local_api_version: LOCAL_API_VERSION,
				capability_hash: RPC_CAPABILITY_HASH,
			});
			expect(health).toMatchObject({
				result: { spool: { status: "critical", warnings: ["normal spool usage reached 80%"] } },
			});
		} finally {
			await daemon.stop();
		}

		const memorySecret = `ghp_${"B".repeat(36)}`;
		const memoryResult = spoolMutation(
			{
				method: "POST /v1/memories/record",
				idempotencyKey: "memory-warning",
				body: {
					idempotencyKey: "memory-warning",
					kind: "decision",
					title: "visible <private>hidden title</private>",
					body: `keep ${memorySecret} <local-only>device only</local-only>`,
				},
			},
			{ dataDir, onWarning: () => {} },
		);
		expect(memoryResult.status).toBe("queued");
		const memoryPersisted = readFileSync(memoryResult.path as string, "utf8");
		expect(memoryPersisted).not.toContain(memorySecret);
		expect(memoryPersisted).not.toContain("hidden title");
		expect(JSON.parse(memoryPersisted)).toMatchObject({
			redaction: {
				sensitivity: "secret",
				private_content_omitted: true,
				local_only: true,
			},
		});

		writeSparse(join(layout.tmpDir, "normal-full.json.tmp"), SPOOL_NORMAL_QUOTA_BYTES);
		const normalDrop = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "normal-full",
				body: eventBody("normal-full"),
			},
			{ dataDir, onWarning: () => {} },
		);
		const reserved = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "reserved",
				body: eventBody("reserved", "pre_compact"),
			},
			{ dataDir, onWarning: () => {} },
		);
		expect(normalDrop).toMatchObject({ status: "dropped", reason: "quota_full" });
		expect(reserved).toMatchObject({ status: "queued", quotaClass: "reserved" });

		writeSparse(join(layout.tmpDir, "reserved-full.json.tmp"), SPOOL_RESERVED_QUOTA_BYTES);
		const reservedDrop = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "reserved-full",
				body: eventBody("reserved-full", "session_ended"),
			},
			{ dataDir, onWarning: () => {} },
		);
		const status = readSpoolStatus(dataDir);
		expect(reservedDrop).toMatchObject({ status: "dropped", reason: "quota_full" });
		expect(status.dropped.byKind).toMatchObject({ tool_completed: 1, session_ended: 1 });
		expect(statSync(layout.counterPath).size).toBe(4096);
		expect(existsSync(join(layout.tmpDir, "normal-full.json.tmp"))).toBe(true);
		const criticalDaemon = await startDaemon({ dataDir });
		try {
			const doctor = await callDaemonRpc(criticalDaemon.socketPath, {
				id: "spool-doctor",
				method: "GET /v1/doctor",
				adapter_version: "test",
				native_cli_version: "test",
				normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
				local_api_version: LOCAL_API_VERSION,
				capability_hash: RPC_CAPABILITY_HASH,
			});
			expect(doctor).toMatchObject({
				result: {
					diagnostics: {
						spool: {
							status: "critical",
							droppedTotal: 2,
							droppedByKind: { tool_completed: 1, session_ended: 1 },
						},
					},
				},
			});
		} finally {
			await criticalDaemon.stop();
		}

		const quarantineDataDir = await tempDataDir();
		const first = spoolMutation(
			{ method: "POST /v1/events", idempotencyKey: "q1", body: eventBody("q1") },
			{ dataDir: quarantineDataDir, onWarning: () => {} },
		);
		const second = spoolMutation(
			{ method: "POST /v1/events", idempotencyKey: "q2", body: eventBody("q2") },
			{ dataDir: quarantineDataDir, onWarning: () => {} },
		);
		expect(
			quarantineSpoolEntry(quarantineDataDir, basename(first.path as string), "broken_json"),
		).toMatchObject({ status: "moved" });
		const quarantineLayout = resolveSpoolLayout(quarantineDataDir);
		writeSparse(join(quarantineLayout.quarantineDir, "full.json"), SPOOL_QUARANTINE_QUOTA_BYTES);
		expect(
			quarantineSpoolEntry(
				quarantineDataDir,
				basename(second.path as string),
				"idempotency_conflict",
			),
		).toEqual({ status: "full" });
		expect(existsSync(second.path as string)).toBe(true);
		expect(readSpoolStatus(quarantineDataDir)).toMatchObject({
			critical: true,
			dropped: { quarantineRejected: 1 },
		});
		expect(() =>
			quarantineSpoolEntry(quarantineDataDir, "entry.json", "../escape" as "broken_json"),
		).toThrow("Invalid spool quarantine request");
	});

	it("P1-T039-02-concurrent-writers", async () => {
		const dataDir = await tempDataDir();
		const results = await Promise.all(Array.from({ length: 6 }, () => runWriter(dataDir, "same")));
		const unexpected = results.filter(
			(result) =>
				result.status !== "queued" &&
				result.status !== "duplicate" &&
				!(result.status === "dropped" && result.reason === "lock_timeout"),
		);
		expect(unexpected).toEqual([]);
		expect(results.filter((result) => result.status === "queued")).toHaveLength(1);
		const layout = resolveSpoolLayout(dataDir);
		const ready = readdirSync(layout.readyDir).filter((name) => name.endsWith(".json"));
		expect(ready).toHaveLength(1);
		expect(() =>
			JSON.parse(readFileSync(join(layout.readyDir, ready[0] as string), "utf8")),
		).not.toThrow();

		const tagged = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "redaction-conflict",
				body: eventBody("redaction-conflict", "tool_completed", {
					text: "keep <local-only>device</local-only>",
				}),
			},
			{ dataDir, onWarning: () => {} },
		);
		const plain = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "redaction-conflict",
				body: eventBody("redaction-conflict", "tool_completed", { text: "keep device" }),
			},
			{ dataDir, onWarning: () => {} },
		);
		expect(tagged.status).toBe("queued");
		expect(plain.status).toBe("queued");
		expect(readdirSync(layout.readyDir).filter((name) => name.endsWith(".json"))).toHaveLength(3);

		const held = acquireSpoolLock(dataDir);
		const started = performance.now();
		const timedOut = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "deadline",
				body: eventBody("deadline"),
			},
			{ dataDir, lockDeadlineMs: 20, onWarning: () => {} },
		);
		const elapsed = performance.now() - started;
		held.close();
		expect(timedOut).toMatchObject({ status: "dropped", reason: "lock_timeout" });
		expect(elapsed).toBeLessThan(250);

		const replaced = acquireSpoolLock(dataDir);
		unlinkSync(layout.lockPath);
		const replacement = acquireSpoolLock(dataDir);
		replaced.close();
		expect(existsSync(layout.lockPath)).toBe(true);
		const replacementStillHeld = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "replacement-lock",
				body: eventBody("replacement-lock"),
			},
			{ dataDir, lockDeadlineMs: 20, onWarning: () => {} },
		);
		replacement.close();
		expect(replacementStillHeld).toMatchObject({
			status: "dropped",
			reason: "lock_timeout",
		});
	});

	it("P1-T039-06-stale-lock-restat-missing-retries", async () => {
		const dataDir = await tempDataDir();
		const layout = resolveSpoolLayout(dataDir);
		mkdirSync(dirname(layout.lockPath), { recursive: true });
		// A lock owned by a process that no longer exists: removeStaleLock passes its liveness
		// and grace checks and reaches the second stat instead of bailing out early.
		const deadOwner = {
			version: 1,
			pid: 4194303,
			startTime: "0",
			fingerprint: "0",
			nonce: "0",
		};
		writeFileSync(layout.lockPath, `${JSON.stringify(deadOwner)}\n`, { mode: 0o600 });
		fsFault.lstatMissingOnce = layout.lockPath;
		fsFault.lstatMissingArmOnRead = `/proc/${deadOwner.pid}/`;
		const result = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "stale-restat",
				body: eventBody("stale-restat"),
			},
			{ dataDir, onWarning: () => {} },
		);
		// Both empty means the fault armed on the liveness probe and then fired on the stat.
		// Without them the assertions below would hold for a run that never met the fault.
		expect(fsFault.lstatMissingArmOnRead).toBe("");
		expect(fsFault.lstatMissingOnce).toBe("");
		expect(result).toMatchObject({ status: "queued" });
		expect(existsSync(layout.lockPath)).toBe(false);
	});

	it("P1-T039-05-lock-publish-missing-retries", async () => {
		const dataDir = await tempDataDir();
		const layout = resolveSpoolLayout(dataDir);
		fsFault.lstatMissingOnce = layout.lockPath;
		expect(() => acquireSpoolLock(dataDir).close()).not.toThrow();
		// Empty means the fault fired; otherwise the acquire never met it and proves nothing.
		expect(fsFault.lstatMissingOnce).toBe("");
		expect(existsSync(layout.lockPath)).toBe(false);
	});

	it("P1-T039-07-lock-publish-io-error-surfaces", async () => {
		const dataDir = await tempDataDir();
		const layout = resolveSpoolLayout(dataDir);
		// Only a missing path means "someone raced us". A real device error is not contention:
		// reporting it as one would spend the whole lock deadline and then blame the wrong thing.
		fsFault.lstatMissingOnce = layout.lockPath;
		fsFault.lstatFaultCode = "EIO";
		const result = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "publish-io-error",
				body: eventBody("publish-io-error"),
			},
			{ dataDir, onWarning: () => {} },
		);
		expect(fsFault.lstatMissingOnce).toBe("");
		expect(result).toMatchObject({ status: "dropped", reason: "io_error" });
	});

	it("P1-T039-03-disk-full-temp", async () => {
		const dataDir = await tempDataDir();
		const warnings: string[] = [];
		fsFault.renameDiskFull = true;
		const result = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "disk-full",
				body: eventBody("disk-full", "tool_failed"),
			},
			{ dataDir, onWarning: (message) => warnings.push(message) },
		);
		fsFault.renameDiskFull = false;
		const layout = resolveSpoolLayout(dataDir);
		expect(result).toMatchObject({ status: "dropped", reason: "disk_full" });
		expect(warnings.join("\n")).toContain("disk full");
		expect(readdirSync(layout.tmpDir).some((name) => name.endsWith(".json.tmp"))).toBe(true);
		expect(readSpoolStatus(dataDir).dropped.byKind).toMatchObject({ tool_failed: 1 });
		const invalid = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "invalid",
				body: eventBody("invalid", "not-a-kind"),
			},
			{ dataDir, onWarning: (message) => warnings.push(message) },
		);
		expect(invalid).toMatchObject({ status: "dropped", reason: "invalid" });
		const mismatched = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "outer-key",
				body: eventBody("inner-key"),
			},
			{ dataDir, onWarning: (message) => warnings.push(message) },
		);
		expect(mismatched).toMatchObject({ status: "dropped", reason: "invalid" });
		const paddedReservedKind = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "padded-kind",
				body: eventBody("padded-kind", " pre_compact "),
			},
			{ dataDir, onWarning: (message) => warnings.push(message) },
		);
		expect(paddedReservedKind).toMatchObject({
			status: "dropped",
			quotaClass: "normal",
			reason: "invalid",
		});
		expect(warnings.join("\n")).not.toContain("not-a-kind");
		expect(warnings.join("\n")).not.toContain("inner-key");
	});

	it("does not rerun failed user rules on an already degraded event", async () => {
		const dataDir = await tempDataDir();
		const key = "degraded";
		const result = spoolMutation(
			{ method: "POST /v1/events", idempotencyKey: key, body: eventBody(key) },
			{
				dataDir,
				config: parseAgentMemoryToml('secret_regex = ["event-degraded"]'),
				previousRedaction: {
					sensitivity: "normal",
					secret_rules_version: `${"a".repeat(64)}:degraded`,
					redaction_degraded: true,
					private_content_omitted: false,
					local_only: false,
				},
			},
		);
		expect(result.status).toBe("queued");
		const entry = JSON.parse(readFileSync(result.path as string, "utf8"));
		expect(entry.body.event.eventId).toBe("event-degraded");
		expect(entry.redaction.redaction_degraded).toBe(true);

		const memoryKey = "degraded-memory";
		const privateProject = "PRIVATE_DEGRADED_PROJECT";
		const memory = spoolMutation(
			{
				method: "POST /v1/memories/record",
				idempotencyKey: memoryKey,
				body: {
					idempotencyKey: memoryKey,
					kind: "decision",
					title: "raw title",
					body: "raw body",
					project: privateProject,
				},
			},
			{
				dataDir,
				previousRedaction: {
					sensitivity: "normal",
					secret_rules_version: `${"b".repeat(64)}:degraded`,
					redaction_degraded: true,
					private_content_omitted: false,
					local_only: false,
				},
			},
		);
		expect(memory.status).toBe("queued");
		const serialized = readFileSync(memory.path as string, "utf8");
		expect(serialized).not.toContain(privateProject);
		expect(JSON.parse(serialized)).toMatchObject({
			body: {
				title: "[REDACTED:degraded]",
				body: "[REDACTED:degraded]",
			},
			redaction: { redaction_degraded: true },
		});
	});

	it("keeps a degraded spool rescan over healthy adapter metadata", async () => {
		const dataDir = await tempDataDir();
		const key = "spool-rescan-degraded";
		const secret = `${"a".repeat(26)}!`;
		const previousVersion = "a".repeat(64);
		const result = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: key,
				body: eventBody(key, "tool_completed", { text: secret }),
			},
			{
				dataDir,
				config: parseAgentMemoryToml('secret_regex = ["(a+)+$"]'),
				previousRedaction: {
					sensitivity: "normal",
					secret_rules_version: previousVersion,
					redaction_degraded: false,
					private_content_omitted: false,
					local_only: false,
				},
			},
		);
		expect(result.status).toBe("queued");
		const serialized = readFileSync(result.path as string, "utf8");
		expect(serialized).not.toContain(secret);
		const entry = JSON.parse(serialized);
		expect(entry.body.event.payload).toEqual({});
		expect(entry.redaction).toMatchObject({
			sensitivity: "secret",
			redaction_degraded: true,
		});
		const versions = String(entry.redaction.secret_rules_version).split("+");
		expect(versions).toContain(previousVersion);
		expect(versions.some((version) => version.endsWith(":degraded"))).toBe(true);
		expect(validateSpoolRedaction(entry.redaction)).toEqual(entry.redaction);
	});

	it("P1-T039-04-old-format-drain", async () => {
		const dataDir = await tempDataDir();
		const claudeDir = join(dataDir, "claude-hook-spool");
		const codexDir = join(dataDir, "codex-hook-spool");
		mkdirSync(claudeDir, { recursive: true });
		mkdirSync(codexDir, { recursive: true });
		writeFileSync(join(claudeDir, "hook-1.json"), JSON.stringify({ session_id: "claude" }));
		writeFileSync(join(codexDir, "hook-2.json"), JSON.stringify({ session_id: "codex" }));
		writeFileSync(join(claudeDir, ".hook-tmp-partial.json"), "{");
		writeFileSync(join(codexDir, ".bad-json.json"), "{");
		const seen: string[] = [];

		const result = await drainLegacySpool(dataDir, async (source, payload) => {
			seen.push(`${source}:${String(payload.session_id)}`);
			return source === "claude";
		});

		expect(result).toEqual({ processed: 1, failed: 1 });
		expect(seen).toEqual(["claude:claude", "codex:codex"]);
		expect(existsSync(join(claudeDir, "hook-1.json"))).toBe(false);
		expect(existsSync(join(codexDir, "hook-2.json"))).toBe(true);
		expect(existsSync(join(claudeDir, ".hook-tmp-partial.json"))).toBe(true);
		expect(existsSync(join(codexDir, ".bad-json.json"))).toBe(true);
	});

	it("P1-T055-01-spool-fault-boundaries", async () => {
		const writeDataDir = await tempDataDir();
		fsFault.tmpWrite = true;
		expect(
			spoolMutation(
				{
					method: "POST /v1/events",
					idempotencyKey: "fault-write",
					body: eventBody("fault-write"),
				},
				{ dataDir: writeDataDir, onWarning: () => {} },
			),
		).toMatchObject({ status: "dropped", reason: "io_error" });
		fsFault.tmpWrite = false;
		const writeLayout = resolveSpoolLayout(writeDataDir);
		expect(readdirSync(writeLayout.tmpDir)).toEqual([]);
		expect(readdirSync(writeLayout.readyDir)).toEqual([]);

		const fileSyncDataDir = await tempDataDir();
		fsFault.flushWrite = true;
		expect(
			spoolMutation(
				{
					method: "POST /v1/events",
					idempotencyKey: "fault-file-fsync",
					body: eventBody("fault-file-fsync"),
				},
				{ dataDir: fileSyncDataDir, onWarning: () => {} },
			),
		).toMatchObject({ status: "dropped", reason: "io_error" });
		const fileSyncLayout = resolveSpoolLayout(fileSyncDataDir);
		expect(readdirSync(fileSyncLayout.tmpDir)).toHaveLength(1);
		fsFault.flushWrite = false;
		fsFault.fsyncSuffix = ".json.tmp";
		expect(
			spoolMutation(
				{
					method: "POST /v1/events",
					idempotencyKey: "fault-file-fsync",
					body: eventBody("fault-file-fsync"),
				},
				{ dataDir: fileSyncDataDir, onWarning: () => {} },
			),
		).toMatchObject({ status: "dropped", reason: "io_error" });
		expect(readdirSync(fileSyncLayout.tmpDir)).toHaveLength(1);
		const recoveryFaulted = await startDaemon({ dataDir: fileSyncDataDir });
		await recoveryFaulted.stop();
		expect(readdirSync(fileSyncLayout.tmpDir)).toHaveLength(1);
		expect(readdirSync(fileSyncLayout.readyDir)).toEqual([]);
		fsFault.fsyncSuffix = "";
		const recovered = await startDaemon({ dataDir: fileSyncDataDir });
		await recovered.stop();
		expect(readdirSync(fileSyncLayout.tmpDir)).toEqual([]);
		expect(readdirSync(fileSyncLayout.readyDir)).toEqual([]);
		expectExactlyOneEvent(fileSyncDataDir, "fault-file-fsync");

		for (const directory of ["tmp", "ready"] as const) {
			const dataDir = await tempDataDir();
			const layout = resolveSpoolLayout(dataDir);
			fsFault.fsyncSuffix = `/control/spool/${directory}`;
			const key = `fault-${directory}-fsync`;
			expect(
				spoolMutation(
					{ method: "POST /v1/events", idempotencyKey: key, body: eventBody(key) },
					{ dataDir, onWarning: () => {} },
				),
			).toMatchObject({ status: "dropped", reason: "io_error" });
			expect(readdirSync(layout.readyDir)).toHaveLength(1);
			expect(
				spoolMutation(
					{ method: "POST /v1/events", idempotencyKey: key, body: eventBody(key) },
					{ dataDir, onWarning: () => {} },
				),
			).toMatchObject({ status: "dropped", reason: "io_error" });
			expect(readdirSync(layout.readyDir)).toHaveLength(1);
			fsFault.fsyncSuffix = "";
			expect(
				spoolMutation(
					{ method: "POST /v1/events", idempotencyKey: key, body: eventBody(key) },
					{ dataDir, onWarning: () => {} },
				),
			).toMatchObject({ status: "duplicate" });
		}

		const deleteDataDir = await tempDataDir();
		const queued = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "fault-delete",
				body: eventBody("fault-delete"),
			},
			{ dataDir: deleteDataDir, onWarning: () => {} },
		);
		expect(queued.status).toBe("queued");
		fsFault.readyUnlink = true;
		const first = await startDaemon({ dataDir: deleteDataDir });
		await first.stop();
		expect(existsSync(queued.path as string)).toBe(true);
		fsFault.readyUnlink = false;
		const second = await startDaemon({ dataDir: deleteDataDir });
		await second.stop();
		expect(existsSync(queued.path as string)).toBe(false);
		expectExactlyOneEvent(deleteDataDir, "fault-delete");

		const deleteSyncDataDir = await tempDataDir();
		const deleteSyncQueued = spoolMutation(
			{
				method: "POST /v1/events",
				idempotencyKey: "fault-delete-fsync",
				body: eventBody("fault-delete-fsync"),
			},
			{ dataDir: deleteSyncDataDir, onWarning: () => {} },
		);
		expect(deleteSyncQueued.status).toBe("queued");
		const serialized = readFileSync(deleteSyncQueued.path as string, "utf8");
		fsFault.fsyncSuffix = "/control/spool/ready";
		const third = await startDaemon({ dataDir: deleteSyncDataDir });
		await third.stop();
		expect(existsSync(deleteSyncQueued.path as string)).toBe(false);
		writeFileSync(deleteSyncQueued.path as string, serialized, { mode: 0o600 });
		fsFault.fsyncSuffix = "";
		const fourth = await startDaemon({ dataDir: deleteSyncDataDir });
		await fourth.stop();
		expect(existsSync(deleteSyncQueued.path as string)).toBe(false);
		expectExactlyOneEvent(deleteSyncDataDir, "fault-delete-fsync");
	});
});
