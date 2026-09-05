import { existsSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import BetterSqlite3 from "better-sqlite3";
import { afterEach, describe, expect, it } from "vitest";
import { startDaemon } from "./daemon-lifecycle.js";
import {
	callDaemonRpc,
	LOCAL_API_VERSION,
	NORMALIZED_SCHEMA_VERSION,
	RPC_CAPABILITY_HASH,
	type RpcRequest,
} from "./daemon-rpc.js";
import { canonicalMutationJson } from "./mutation-dispatcher.js";
import * as spool from "./spool.js";
import { readCurrentDatabasePointer } from "./storage.js";

const roots: string[] = [];

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function tempDataDir(): Promise<string> {
	const root = await mkdtemp(join(tmpdir(), "codemem-spool-importer-"));
	roots.push(root);
	return join(root, "data");
}

function memoryMutation(idempotencyKey: string, title: string) {
	return {
		method: "POST /v1/memories/record" as const,
		idempotencyKey,
		body: {
			idempotencyKey,
			kind: "decision",
			title,
			body: `${title} body`,
			confidence: 0.8,
			project: "spool-importer-test",
		},
	};
}

function eventMutation(idempotencyKey: string, text = "synthetic producer") {
	return {
		method: "POST /v1/events" as const,
		idempotencyKey,
		body: {
			idempotencyKey,
			event: {
				schemaVersion: 1,
				eventId: `event-${idempotencyKey}`,
				idempotencyKey,
				agent: "codex",
				nativeSessionId: "session-spool-importer",
				projectKey: "spool-importer-test",
				workspaceKey: "workspace-spool-importer",
				cwd: "/tmp/spool-importer",
				kind: "tool_completed",
				occurredAt: "2026-08-14T00:00:00.000Z",
				payload: { text },
				sourceHash: "a".repeat(64),
				sensitivity: "normal",
			},
		},
	};
}

function rpc(method: string, body: Record<string, unknown>, id: string): RpcRequest {
	return {
		id,
		method,
		adapter_version: "test",
		native_cli_version: "test",
		normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
		local_api_version: LOCAL_API_VERSION,
		capability_hash: RPC_CAPABILITY_HASH,
		body,
	};
}

async function waitUntil(
	predicate: () => boolean | Promise<boolean>,
	timeoutMs: number,
): Promise<boolean> {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		if (await predicate()) return true;
		await new Promise((resolve) => setTimeout(resolve, 25));
	}
	return predicate();
}

async function waitForDaemonJobsIdle(socketPath: string): Promise<boolean> {
	return waitUntil(async () => {
		const response = await callDaemonRpc(socketPath, rpc("GET /v1/jobs", {}, "wait-for-jobs"));
		const jobs = (response as { result?: { jobs?: Array<{ state?: string }> } }).result?.jobs;
		return (
			Array.isArray(jobs) &&
			jobs.every((job) => job.state === "completed" || job.state === "failed")
		);
	}, 3_000);
}

describe("phase 1 spool importer", () => {
	it("P1-T040-01-commit-before-delete", async () => {
		const dataDir = await tempDataDir();
		const queued = spool.spoolMutation(memoryMutation("import-unit", "unit"), {
			dataDir,
			onWarning: () => {},
		});
		expect(queued.status).toBe("queued");
		const layout = spool.resolveSpoolLayout(dataDir);
		const readyName = basename(queued.path as string);
		const recoveredReadyPath = join(layout.readyDir, readyName);
		renameSync(recoveredReadyPath, join(layout.tmpDir, `${readyName}.tmp`));

		let attempts = 0;
		spool.importReadySpoolEntries(dataDir, (entry) => {
			attempts++;
			expect(entry).toMatchObject({
				method: "POST /v1/memories/record",
				idempotencyKey: "import-unit",
			});
			expect(existsSync(recoveredReadyPath)).toBe(true);
			throw new Error("synthetic writer failure");
		});
		expect(attempts).toBe(1);
		expect(existsSync(recoveredReadyPath)).toBe(true);

		spool.importReadySpoolEntries(dataDir, () => {
			expect(existsSync(recoveredReadyPath)).toBe(true);
			return "committed";
		});
		expect(existsSync(recoveredReadyPath)).toBe(false);
	});

	it("P1-T040-03-import-conflict", async () => {
		const dataDir = await tempDataDir();
		const variants = [
			memoryMutation("startup-conflict", "first"),
			memoryMutation("startup-conflict", "second"),
		];
		for (const mutation of variants) {
			expect(spool.spoolMutation(mutation, { dataDir, onWarning: () => {} }).status).toBe("queued");
		}
		const layout = spool.resolveSpoolLayout(dataDir);
		writeFileSync(join(layout.readyDir, "normal-broken.json"), "{broken\n", { mode: 0o600 });
		const tampered = spool.spoolMutation(memoryMutation("startup-tampered", "original"), {
			dataDir,
			onWarning: () => {},
		});
		const tamperedEntry = JSON.parse(readFileSync(tampered.path as string, "utf8")) as {
			body: { title: string };
		};
		tamperedEntry.body.title = "tampered";
		writeFileSync(tampered.path as string, `${canonicalMutationJson(tamperedEntry)}\n`);

		const daemon = await startDaemon({ dataDir });
		try {
			expect(readdirSync(layout.readyDir)).toEqual([]);
			const quarantined = readdirSync(layout.quarantineDir);
			expect(quarantined.some((name) => name.startsWith("broken_json-"))).toBe(true);
			expect(quarantined.some((name) => name.startsWith("idempotency_conflict-"))).toBe(true);

			const view = await callDaemonRpc(
				daemon.socketPath,
				rpc("GET /v1/view", { collection: "memories" }, "view-startup-import"),
			);
			expect(view).toHaveProperty("result");
			const memories = (
				view as { result: { body: { items: Array<{ id: number; title: string }> } } }
			).result.body.items;
			expect(memories).toHaveLength(1);
			const committed = variants.find((variant) => variant.body.title === memories[0]?.title);
			expect(committed).toBeDefined();
			expect(await waitForDaemonJobsIdle(daemon.socketPath)).toBe(true);
			const replay = await callDaemonRpc(
				daemon.socketPath,
				rpc("POST /v1/memories/record", committed?.body ?? {}, "replay-startup-import"),
			);
			expect(replay).toMatchObject({ result: { memoryId: memories[0]?.id } });
		} finally {
			await daemon.stop();
		}
	});

	it("P1-T040-05-preserves-event-reconciliation-conflict", async () => {
		const dataDir = await tempDataDir();
		const first = eventMutation("event-reconciliation-conflict", "first payload");
		const initial = await startDaemon({ dataDir });
		try {
			expect(
				await callDaemonRpc(
					initial.socketPath,
					rpc("POST /v1/events", first.body, "event-reconciliation-first"),
				),
			).toMatchObject({ result: { status: "committed" } });
		} finally {
			await initial.stop();
		}

		const conflicting = spool.spoolMutation(
			eventMutation("event-reconciliation-conflict", "second payload"),
			{ dataDir, onWarning: () => {} },
		);
		expect(conflicting.status).toBe("queued");
		const layout = spool.resolveSpoolLayout(dataDir);
		const restarted = await startDaemon({ dataDir });
		try {
			expect(readdirSync(layout.readyDir)).toEqual([]);
			expect(
				readdirSync(layout.quarantineDir).some((name) => name.startsWith("idempotency_conflict-")),
			).toBe(true);
			const pointer = readCurrentDatabasePointer(restarted.layout);
			expect(pointer).not.toBeNull();
			const db = new BetterSqlite3(join(restarted.layout.dbDir, pointer as string), {
				readonly: true,
				fileMustExist: true,
			});
			try {
				expect(db.prepare("SELECT COUNT(*) AS count FROM raw_events").get()).toEqual({ count: 1 });
				expect(db.prepare("SELECT COUNT(*) AS count FROM raw_event_quarantine").get()).toEqual({
					count: 1,
				});
				expect(db.prepare("SELECT COUNT(*) AS count FROM mutation_quarantine").get()).toEqual({
					count: 1,
				});
			} finally {
				db.close();
			}
		} finally {
			await restarted.stop();
		}
	});

	it("P1-T040-02-import-exactly-once", async () => {
		const dataDir = await tempDataDir();
		const daemon = await startDaemon({ dataDir });
		let stopped = false;
		try {
			expect(await waitForDaemonJobsIdle(daemon.socketPath)).toBe(true);
			const mutation = eventMutation("periodic-event");
			const queued = spool.spoolMutation(mutation, { dataDir, onWarning: () => {} });
			expect(queued.status).toBe("queued");
			expect(await waitUntil(() => !existsSync(queued.path as string), 3_000)).toBe(true);
			expect(readdirSync(spool.resolveSpoolLayout(dataDir).quarantineDir)).toEqual([]);

			const replay = await callDaemonRpc(
				daemon.socketPath,
				rpc("POST /v1/events", mutation.body, "replay-periodic-import"),
			);
			expect(replay).toMatchObject({
				result: {
					status: "quarantined",
					safeErrorCode: "repository_identity_unknown_collision",
				},
			});
			await daemon.stop();
			stopped = true;

			const pointer = readCurrentDatabasePointer(daemon.layout);
			expect(pointer).not.toBeNull();
			const db = new BetterSqlite3(join(daemon.layout.dbDir, pointer as string), {
				readonly: true,
				fileMustExist: true,
			});
			try {
				expect(
					db
						.prepare(
							"SELECT COUNT(*) AS count FROM mutation_receipts WHERE method = ? AND idempotency_key = ?",
						)
						.get("POST /v1/events", "periodic-event"),
				).toEqual({ count: 1 });
				expect(
					db
						.prepare("SELECT COUNT(*) AS count FROM raw_events WHERE event_id = ?")
						.get("event-periodic-event"),
				).toEqual({ count: 1 });
			} finally {
				db.close();
			}
		} finally {
			if (!stopped) await daemon.stop();
		}
	});

	it("P1-T040-04-spooled-redaction-metadata-matches-direct-replay", async () => {
		const dataDir = await tempDataDir();
		const secret = "AKIAIOSFODNN7EXAMPLE";
		const mutation = memoryMutation("redacted-spool", "visible <private>hidden</private>");
		mutation.body.body = `credential ${secret} <local-only>device only</local-only>`;
		const queued = spool.spoolMutation(mutation, { dataDir, onWarning: () => {} });
		expect(readFileSync(queued.path as string, "utf8")).not.toContain(secret);
		expect(readFileSync(queued.path as string, "utf8")).not.toContain("hidden");

		const daemon = await startDaemon({ dataDir });
		let stopped = false;
		try {
			const replay = await callDaemonRpc(
				daemon.socketPath,
				rpc("POST /v1/memories/record", mutation.body, "replay-redacted-spool"),
			);
			expect(replay).toMatchObject({ result: { memoryId: expect.any(Number) } });
			const memoryId = (replay as { result: { memoryId: number } }).result.memoryId;
			await daemon.stop();
			stopped = true;

			const pointer = readCurrentDatabasePointer(daemon.layout);
			const db = new BetterSqlite3(join(daemon.layout.dbDir, pointer as string), {
				readonly: true,
				fileMustExist: true,
			});
			try {
				const row = db
					.prepare("SELECT title, body_text, metadata_json FROM memory_items WHERE id = ?")
					.get(memoryId) as { title: string; body_text: string; metadata_json: string };
				expect(row.title).toBe("");
				expect(row.body_text).toBe("");
				expect(row.metadata_json).not.toContain(secret);
				expect(JSON.parse(row.metadata_json)).toMatchObject({
					sensitivity: "secret",
					private_content_omitted: true,
					local_only: true,
				});
				expect(
					db
						.prepare(
							"SELECT COUNT(*) AS count FROM mutation_receipts WHERE method = ? AND idempotency_key = ?",
						)
						.get("POST /v1/memories/record", mutation.idempotencyKey),
				).toEqual({ count: 1 });
			} finally {
				db.close();
			}
		} finally {
			if (!stopped) await daemon.stop();
		}
	});
});
