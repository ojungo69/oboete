import { randomUUID } from "node:crypto";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as p from "@clack/prompts";
import type { MemoryStore } from "@codemem/core";
import {
	callDaemonRpc,
	compileDefaultCapabilityManifest,
	hashMutationPayload,
	LOCAL_API_VERSION,
	NORMALIZED_SCHEMA_VERSION,
	RPC_CAPABILITY_HASH,
	readCurrentDatabasePointer,
	resolveStorageLayout,
	startDaemon,
} from "@codemem/core";
import { describe, expect, it, vi } from "vitest";
import {
	acquireCapabilityLifecycleLock,
	activateCapabilityManifest,
	writeCapabilityManifestGeneration,
} from "../../../core/src/storage.js";
import { openTestMemoryStore } from "../../../core/src/test-utils.js";
import { ReadOnlyActor } from "../../../core/src/writer-actor.js";
import { dbCommand } from "./db.js";

vi.mock("@clack/prompts", async (importOriginal) => ({
	...(await importOriginal<typeof import("@clack/prompts")>()),
	confirm: vi.fn(),
}));

describe("db command", () => {
	it("registers exact-job raw-events-doctor-retry with an interactive confirmation gate", async () => {
		const retry = dbCommand.commands.find(
			(command) => command.name() === "raw-events-doctor-retry",
		);
		expect(retry).toBeDefined();
		if (!retry) throw new Error("expected raw-events-doctor-retry command");
		expect(retry.usage()).toContain("<job-id>");
		expect(retry.options.map((option) => option.long)).toContain("--db-path");
		expect(retry.options.map((option) => option.long)).not.toContain("--limit");
	});

	it("displays exact job fingerprints before confirming one doctor retry grant", async () => {
		const retry = dbCommand.commands.find(
			(command) => command.name() === "raw-events-doctor-retry",
		);
		if (!retry) throw new Error("expected raw-events-doctor-retry command");
		const dataDir = join(mkdtempSync(join(tmpdir(), "codemem-db-doctor-retry-")), "data");
		const initialized = await startDaemon({ dataDir });
		await initialized.stop();
		const layout = resolveStorageLayout(dataDir);
		const pointer = readCurrentDatabasePointer(layout);
		if (!pointer) throw new Error("canonical database pointer is missing");
		const manifest = compileDefaultCapabilityManifest({
			version: 1,
			role: "summary",
			state: "enabled",
			wireProtocol: "openai_chat_completions_v1",
			modelId: "doctor-cli",
			modelRevision: "1",
			endpointUrl: "http://127.0.0.1:1234/v1/chat/completions",
			credentialRef: { kind: "none" },
		});
		writeCapabilityManifestGeneration(layout, manifest);
		const lifecycle = acquireCapabilityLifecycleLock(layout);
		try {
			activateCapabilityManifest(layout, manifest.configurationFingerprint, lifecycle);
		} finally {
			lifecycle.close();
		}
		const store = openTestMemoryStore(join(layout.dbDir, pointer));
		const manifestFingerprint = manifest.configurationFingerprint;
		const providerFingerprint = manifest.summaryProvider.providerFingerprint;
		let jobId: number;
		try {
			store.recordRawEvent({
				opencodeSessionId: "doctor-cli",
				source: "codex",
				eventId: "doctor-cli-0",
				eventType: "user_prompt",
				payload: { text: "legacy recovery source" },
				repositoryIdentity: `repo-v1:sha256:${"a".repeat(64)}`,
				sensitivity: "eligible",
			});
			const inserted = store.db
				.prepare(
					`INSERT INTO raw_event_flush_batches(
						source, stream_id, opencode_session_id, start_event_seq, end_event_seq,
						extractor_version, status, admission_manifest_fingerprint,
						admission_provider_fingerprint, attempt_manifest_fingerprint,
						attempt_provider_fingerprint, retry_limit, attempt_count,
						claim_generation, legacy_recovery_state, created_at, updated_at
					 ) VALUES ('codex', 'doctor-cli', 'doctor-cli', 0, 0, 'raw_events_v1',
						'retry_exhausted', NULL, NULL, NULL, NULL, 3, 3, 0,
						'complete_range', ?, ?)`,
				)
				.run("2026-08-31T00:00:00.000Z", "2026-08-31T00:00:00.000Z");
			jobId = Number(inserted.lastInsertRowid);
		} finally {
			store.close();
		}
		const daemon = await startDaemon({ dataDir });
		const priorDataDir = process.env.CODEMEM_DATA_DIR;
		const priorExitCode = process.exitCode;
		const info = vi.spyOn(p.log, "info").mockImplementation(() => {});
		const error = vi.spyOn(p.log, "error").mockImplementation(() => {});
		const success = vi.spyOn(p.log, "success").mockImplementation(() => {});
		const confirm = vi
			.mocked(p.confirm)
			.mockRejectedValueOnce(new Error("injected prompt failure"))
			.mockResolvedValue(true);
		try {
			process.env.CODEMEM_DATA_DIR = dataDir;
			process.exitCode = undefined;
			await retry.parseAsync(["node", "raw-events-doctor-retry", String(jobId)], {
				from: "node",
			});
			expect(error).toHaveBeenCalledWith("Retry confirmation failed.");
			expect(success).not.toHaveBeenCalled();
			expect(process.exitCode).toBe(1);

			process.exitCode = undefined;
			await retry.parseAsync(["node", "raw-events-doctor-retry", String(jobId)], {
				from: "node",
			});
			expect(info).toHaveBeenCalledWith(expect.stringContaining(manifestFingerprint));
			expect(info).toHaveBeenCalledWith(expect.stringContaining(providerFingerprint));
			expect(info).toHaveBeenCalledWith(expect.stringContaining("legacy_unknown"));
			expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ initialValue: false }));
			expect(success).toHaveBeenCalled();
			expect(process.exitCode).toBeUndefined();
		} finally {
			if (priorDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
			else process.env.CODEMEM_DATA_DIR = priorDataDir;
			process.exitCode = priorExitCode;
			confirm.mockReset();
			info.mockRestore();
			error.mockRestore();
			success.mockRestore();
			await daemon.stop();
		}
	});

	it("registers backfill-tags maintenance subcommand", () => {
		const backfill = dbCommand.commands.find((command) => command.name() === "backfill-tags");
		expect(backfill).toBeDefined();
		const longs = backfill?.options.map((option) => option.long) ?? [];
		expect(longs).toContain("--db");
		expect(longs).toContain("--db-path");
		expect(longs).toContain("--limit");
		expect(longs).toContain("--since");
		expect(longs).toContain("--project");
		expect(longs).toContain("--all-projects");
		expect(longs).toContain("--inactive");
		expect(longs).toContain("--dry-run");
		expect(longs).toContain("--json");
	});

	it("registers prune-observations and prune-memories subcommands", () => {
		const pruneObs = dbCommand.commands.find((command) => command.name() === "prune-observations");
		const pruneMem = dbCommand.commands.find((command) => command.name() === "prune-memories");
		expect(pruneObs).toBeDefined();
		expect(pruneMem).toBeDefined();

		const pruneObsLongs = pruneObs?.options.map((option) => option.long) ?? [];
		expect(pruneObsLongs).toContain("--limit");
		expect(pruneObsLongs).toContain("--dry-run");
		expect(pruneObsLongs).toContain("--json");

		const pruneMemLongs = pruneMem?.options.map((option) => option.long) ?? [];
		expect(pruneMemLongs).toContain("--limit");
		expect(pruneMemLongs).toContain("--kinds");
		expect(pruneMemLongs).toContain("--dry-run");
		expect(pruneMemLongs).toContain("--json");
	});

	it("registers dedup-memories, backfill-dedup-keys, backfill-narrative, and ai-backfill-structured subcommands", () => {
		const dedup = dbCommand.commands.find((command) => command.name() === "dedup-memories");
		const dedupKeys = dbCommand.commands.find(
			(command) => command.name() === "backfill-dedup-keys",
		);
		const narrative = dbCommand.commands.find((command) => command.name() === "backfill-narrative");
		const aiStructured = dbCommand.commands.find(
			(command) => command.name() === "ai-backfill-structured",
		);
		expect(dedup).toBeDefined();
		expect(dedupKeys).toBeDefined();
		expect(narrative).toBeDefined();
		expect(aiStructured).toBeDefined();

		const dedupLongs = dedup?.options.map((option) => option.long) ?? [];
		expect(dedupLongs).toContain("--window");
		expect(dedupLongs).toContain("--limit");
		expect(dedupLongs).toContain("--dry-run");
		expect(dedupLongs).toContain("--json");

		const dedupKeysLongs = dedupKeys?.options.map((option) => option.long) ?? [];
		expect(dedupKeysLongs).toContain("--limit");
		expect(dedupKeysLongs).toContain("--dry-run");
		expect(dedupKeysLongs).toContain("--json");

		const narrativeLongs = narrative?.options.map((option) => option.long) ?? [];
		expect(narrativeLongs).toContain("--limit");
		expect(narrativeLongs).toContain("--dry-run");
		expect(narrativeLongs).toContain("--json");

		const aiLongs = aiStructured?.options.map((option) => option.long) ?? [];
		expect(aiLongs).toContain("--limit");
		expect(aiLongs).toContain("--kinds");
		expect(aiLongs).toContain("--overwrite");
		expect(aiLongs).toContain("--dry-run");
		expect(aiLongs).toContain("--json");
	});

	it("registers prune-raw-events subcommand with age-based options", () => {
		const pruneRaw = dbCommand.commands.find((command) => command.name() === "prune-raw-events");
		expect(pruneRaw).toBeDefined();
		const longs = pruneRaw?.options.map((option) => option.long) ?? [];
		expect(longs).toContain("--db-path");
		expect(longs).toContain("--dry-run");
		expect(longs).toContain("--max-age-days");
		expect(longs).toContain("--vacuum");
		// Age-based only: no size-budget/batch options.
		expect(longs).not.toContain("--max-size-mb");
		expect(longs).not.toContain("--batch-ops");
	});

	function seedRawEvent(store: MemoryStore, sessionId: string, eventId: string, tsWallMs: number) {
		store.recordRawEvent({
			opencodeSessionId: sessionId,
			eventId,
			eventType: "user_prompt",
			payload: { type: "user_prompt", prompt_text: `seed ${eventId}` },
			tsWallMs,
		});
	}

	function countRawEvents(dbPath: string): number {
		const db = ReadOnlyActor.open(dbPath);
		try {
			const row = db.prepare("SELECT COUNT(*) AS cnt FROM raw_events").get() as {
				cnt: number;
			};
			return Number(row.cnt);
		} finally {
			db.close();
		}
	}

	async function seedDaemonRawEvent(
		socketPath: string,
		sessionId: string,
		eventId: string,
		tsWallMs: number,
	): Promise<void> {
		const response = await callDaemonRpc(socketPath, {
			id: randomUUID(),
			method: "POST /v1/events",
			adapter_version: "test",
			native_cli_version: "test",
			normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
			local_api_version: LOCAL_API_VERSION,
			capability_hash: RPC_CAPABILITY_HASH,
			body: {
				idempotencyKey: eventId,
				event: {
					schemaVersion: NORMALIZED_SCHEMA_VERSION,
					eventId,
					idempotencyKey: eventId,
					agent: "opencode",
					nativeSessionId: sessionId,
					projectKey: "db-test",
					workspaceKey: "db-test",
					cwd: process.cwd(),
					kind: "user_prompted",
					occurredAt: new Date(tsWallMs).toISOString(),
					payload: { text: `seed ${eventId}` },
					sourceHash: hashMutationPayload({ eventId }),
					sensitivity: "normal",
				},
			},
		});
		if ("error" in response) throw new Error(response.error.code);
	}

	async function daemonRawEventCount(socketPath: string): Promise<number> {
		const response = await callDaemonRpc(socketPath, {
			id: randomUUID(),
			method: "GET /v1/view",
			adapter_version: "test",
			native_cli_version: "test",
			normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
			local_api_version: LOCAL_API_VERSION,
			capability_hash: RPC_CAPABILITY_HASH,
			body: { collection: "raw-events" },
		});
		if ("error" in response) throw new Error(response.error.code);
		return Number((response.result.body as { pending?: number }).pending ?? 0);
	}

	it("prune-raw-events --dry-run deletes nothing", async () => {
		const pruneRaw = dbCommand.commands.find((command) => command.name() === "prune-raw-events");
		expect(pruneRaw).toBeDefined();
		if (!pruneRaw) throw new Error("expected prune-raw-events command");

		const dataDir = join(mkdtempSync(join(tmpdir(), "codemem-db-prune-raw-")), "data");
		const daemon = await startDaemon({ dataDir });
		const oldTs = Date.now() - 200 * 86_400_000; // well past a 1-day cutoff
		const originalDataDir = process.env.CODEMEM_DATA_DIR;
		const originalExitCode = process.exitCode;
		try {
			process.env.CODEMEM_DATA_DIR = dataDir;
			process.exitCode = 0;
			await seedDaemonRawEvent(daemon.socketPath, "sess-dry", "evt-0", oldTs);
			await seedDaemonRawEvent(daemon.socketPath, "sess-dry", "evt-1", oldTs + 1000);
			expect(await daemonRawEventCount(daemon.socketPath)).toBe(2);

			await pruneRaw.parseAsync(
				[
					"node",
					"prune-raw-events",
					"--db-path",
					join(dataDir, "legacy.sqlite"),
					"--max-age-days",
					"1",
					"--dry-run",
				],
				{ from: "node" },
			);

			expect(process.exitCode).toBe(0);
			expect(await daemonRawEventCount(daemon.socketPath)).toBe(2);
		} finally {
			if (originalDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
			else process.env.CODEMEM_DATA_DIR = originalDataDir;
			process.exitCode = originalExitCode;
			await daemon.stop();
		}
	});

	it("prune-raw-events retains accepted source events while Slice 1 retention is disabled", async () => {
		const pruneRaw = dbCommand.commands.find((command) => command.name() === "prune-raw-events");
		expect(pruneRaw).toBeDefined();
		if (!pruneRaw) throw new Error("expected prune-raw-events command");

		const dataDir = join(mkdtempSync(join(tmpdir(), "codemem-db-prune-raw-")), "data");
		const daemon = await startDaemon({ dataDir });
		const now = Date.now();
		const originalDataDir = process.env.CODEMEM_DATA_DIR;
		const originalExitCode = process.exitCode;
		try {
			process.env.CODEMEM_DATA_DIR = dataDir;
			process.exitCode = 0;
			await seedDaemonRawEvent(daemon.socketPath, "sess-old", "evt-0", now - 10 * 86_400_000);
			await seedDaemonRawEvent(daemon.socketPath, "sess-old", "evt-1", now - 5 * 86_400_000);
			await seedDaemonRawEvent(daemon.socketPath, "sess-new", "evt-2", now - 1000);
			expect(await daemonRawEventCount(daemon.socketPath)).toBe(3);

			await pruneRaw.parseAsync(
				[
					"node",
					"prune-raw-events",
					"--db-path",
					join(dataDir, "legacy.sqlite"),
					"--max-age-days",
					"1",
				],
				{ from: "node" },
			);

			expect(process.exitCode).toBe(0);
			expect(await daemonRawEventCount(daemon.socketPath)).toBe(3);
		} finally {
			if (originalDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
			else process.env.CODEMEM_DATA_DIR = originalDataDir;
			process.exitCode = originalExitCode;
			await daemon.stop();
		}
		const layout = resolveStorageLayout(dataDir);
		const pointer = readCurrentDatabasePointer(layout);
		if (!pointer) throw new Error("canonical database pointer is missing");
		const reader = ReadOnlyActor.open(join(layout.dbDir, pointer));
		try {
			const remaining = reader.prepare("SELECT event_id FROM raw_events").all() as Array<{
				event_id: string;
			}>;
			expect(remaining.map((row) => row.event_id).toSorted()).toEqual(["evt-0", "evt-1", "evt-2"]);
		} finally {
			reader.close();
		}
	});

	it("prune-raw-events rejects invalid --max-age-days and deletes nothing", async () => {
		const pruneRaw = dbCommand.commands.find((command) => command.name() === "prune-raw-events");
		expect(pruneRaw).toBeDefined();
		if (!pruneRaw) throw new Error("expected prune-raw-events command");

		const dbPath = join(mkdtempSync(join(tmpdir(), "codemem-db-prune-raw-bad-")), "test.sqlite");
		const store = openTestMemoryStore(dbPath);
		seedRawEvent(store, "sess-x", "evt-0", Date.now() - 10 * 86_400_000);
		store.close();
		expect(countRawEvents(dbPath)).toBe(1);

		const logErrorSpy = vi.spyOn(p.log, "error").mockImplementation(() => {});
		const originalExitCode = process.exitCode;
		try {
			// A mistyped age and an explicit 0 must both be rejected — a destructive
			// prune must never run on invalid input. Includes partially-numeric
			// values ("1foo"/"1.5") that Number.parseInt would silently accept as 1.
			for (const bad of ["foo", "0", "1foo", "1.5", "-1", ""]) {
				process.exitCode = undefined;
				await pruneRaw.parseAsync(
					["node", "prune-raw-events", "--db-path", dbPath, "--max-age-days", bad],
					{ from: "node" },
				);
				expect(process.exitCode).toBe(1);
				expect(countRawEvents(dbPath)).toBe(1);
			}
		} finally {
			process.exitCode = originalExitCode;
			logErrorSpy.mockRestore();
		}
	});

	it("prune-raw-events reports a clean error (no uncaught throw) on an unreadable DB", async () => {
		const pruneRaw = dbCommand.commands.find((command) => command.name() === "prune-raw-events");
		expect(pruneRaw).toBeDefined();
		if (!pruneRaw) throw new Error("expected prune-raw-events command");

		// A non-SQLite file makes MemoryStore construction throw; the handler must
		// catch it and set exit code 1 rather than let an uncaught error escape.
		const badDbPath = join(mkdtempSync(join(tmpdir(), "codemem-db-badopen-")), "not-a.sqlite");
		writeFileSync(badDbPath, "this is definitely not a sqlite database");
		const logErrorSpy = vi.spyOn(p.log, "error").mockImplementation(() => {});
		const originalExitCode = process.exitCode;
		process.exitCode = undefined;
		try {
			await expect(
				pruneRaw.parseAsync(
					["node", "prune-raw-events", "--db-path", badDbPath, "--max-age-days", "30"],
					{ from: "node" },
				),
			).resolves.toBeDefined();
			expect(process.exitCode).toBe(1);
		} finally {
			process.exitCode = originalExitCode;
			logErrorSpy.mockRestore();
		}
	});

	it("rejects invalid dedup window input", async () => {
		const dedup = dbCommand.commands.find((command) => command.name() === "dedup-memories");
		expect(dedup).toBeDefined();
		if (!dedup) throw new Error("expected dedup-memories command");

		const dbPath = join(mkdtempSync(join(tmpdir(), "codemem-db-cmd-")), "test.sqlite");
		openTestMemoryStore(dbPath).close();
		const logErrorSpy = vi.spyOn(p.log, "error").mockImplementation(() => {});
		const originalExitCode = process.exitCode;
		process.exitCode = undefined;
		try {
			await dedup.parseAsync(["node", "dedup-memories", "--db-path", dbPath, "--window", "foo"], {
				from: "node",
			});
			expect(process.exitCode).toBe(1);
		} finally {
			process.exitCode = originalExitCode;
			logErrorSpy.mockRestore();
		}
	});
});
