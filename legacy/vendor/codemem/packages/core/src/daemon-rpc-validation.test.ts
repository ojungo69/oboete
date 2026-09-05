import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { captureOnlyCapabilityProjection } from "./capability-manifest.js";
import {
	type DaemonRpcContext,
	dispatchDaemonRpc,
	LOCAL_API_VERSION,
	NORMALIZED_SCHEMA_VERSION,
	RPC_CAPABILITY_HASH,
} from "./daemon-rpc.js";
import { openTestMemoryStore } from "./test-utils.js";

// Two id guards sit on the same request and are easy to confuse: the envelope
// guard (`id` must be a non-empty string) and requirePositiveInt (`body.ids`
// entries must be positive integers). Each is pinned separately below.
function withContext(run: (ctx: DaemonRpcContext, seededIds: number[]) => Promise<void>) {
	const dir = mkdtempSync(join(tmpdir(), "codemem-rpc-id-"));
	let store: ReturnType<typeof openTestMemoryStore> | undefined;
	const cleanup = () => {
		store?.close();
		rmSync(dir, { recursive: true, force: true });
	};
	// Everything after mkdtemp sits inside the try: openTestMemoryStore runs the
	// migrations, and these hand-written INSERTs bypass store.remember — a schema
	// change can make either throw. Outside, that would orphan the temp dir and
	// the sqlite handle, because `.finally` is not attached yet.
	try {
		store = openTestMemoryStore(join(dir, "test.sqlite"));
		const db = store.db;
		const sessionId = Number(
			db
				.prepare(
					`INSERT INTO sessions(started_at, cwd, project, user, tool_version, metadata_json, import_key)
					 VALUES ('2026-03-01T10:00:00Z', '/tmp/repo', 'codemem', 'test-user', 'test', '{}', 'rpc-id-session')`,
				)
				.run().lastInsertRowid,
		);
		const seededIds = ["first", "second"].map((slug) =>
			Number(
				db
					.prepare(
						`INSERT INTO memory_items(
							session_id, kind, title, body_text, confidence, tags_text, active,
							created_at, updated_at, metadata_json, rev, visibility, import_key, sensitivity
						 ) VALUES (?, 'decision', ?, 'seeded for id validation', 0.9, '', 1,
							'2026-03-01T10:00:00Z', '2026-03-01T10:00:00Z', '{}', 1, 'shared', ?, 'eligible')`,
					)
					.run(sessionId, `RPC id fixture ${slug}`, `rpc-id-${slug}`).lastInsertRowid,
			),
		);
		const ctx = {
			identity: { pid: process.pid, nonce: "rpc-id-test" },
			dataDir: dir,
			onStop: () => {},
			writer: db,
			store,
			capability: captureOnlyCapabilityProjection("ready"),
			jobs: { isMaintenanceMode: () => false } as never,
		} as DaemonRpcContext;
		return run(ctx, seededIds).finally(cleanup);
	} catch (error) {
		cleanup();
		throw error;
	}
}

// `requestId` becomes the class-A idempotency key for POST /v1/search. A repeat
// under the same key replays the stored receipt when the payload hash matches and
// raises MutationConflictError when it does not — which is what makes the shared
// key in the rejection test a real ordering probe.
const getMany = (requestId: string, memoryId: unknown, envelopeId: unknown = requestId) =>
	JSON.stringify({
		id: envelopeId,
		method: "POST /v1/search",
		adapter_version: "test",
		native_cli_version: "test",
		normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
		local_api_version: LOCAL_API_VERSION,
		capability_hash: RPC_CAPABILITY_HASH,
		body: { requestId, mode: "get_many", ids: [memoryId] },
	});

describe("daemon RPC id validation", () => {
	it("parses a canonical positive integer string back to the memory it names", async () => {
		await withContext(async (ctx, seededIds) => {
			// Asks for the SECOND seeded row, so a parser that returns a constant
			// (or the first id) answers with the wrong memory rather than an empty list.
			const [, second] = seededIds;
			const response = await dispatchDaemonRpc(getMany("rpc-id-accept", String(second)), ctx);
			expect(response).toMatchObject({ result: { items: [{ id: second }] } });
			expect((response as { result: { items: unknown[] } }).result.items).toHaveLength(1);
		});
	});

	it("rejects non-canonical id spellings before the idempotency key is consumed", async () => {
		await withContext(async (ctx, seededIds) => {
			const [first, second] = seededIds;
			// One key for the whole block. The first call commits a class-A receipt
			// under it, so every later rejection also proves body validation runs
			// before the idempotency lookup: reordered, these differing payloads
			// would come back as a conflict instead of "id is invalid."
			const key = "rpc-id-order";
			expect(await dispatchDaemonRpc(getMany(key, String(second)), ctx)).toMatchObject({
				result: { items: [{ id: second }] },
			});
			// The strings exercise requirePositiveInt's regex branch; the two bare
			// numbers exercise its numeric branch, which nothing else pins.
			for (const memoryId of ["0", "01", "-1", "1.5", "", true, 0, 1.5]) {
				expect(await dispatchDaemonRpc(getMany(key, memoryId), ctx)).toMatchObject({
					error: { code: "invalid_request", message: "id is invalid." },
				});
			}
			// The probe only bites while `ids` is part of the hashed class-A payload.
			// Pin that: the same key with a different valid id must conflict, not
			// replay the receipt above.
			expect(await dispatchDaemonRpc(getMany(key, String(first)), ctx)).toMatchObject({
				error: { code: "idempotency_conflict" },
			});
			// ...and one rejection under a never-used key, so the guard is covered
			// where no receipt exists either.
			expect(await dispatchDaemonRpc(getMany("rpc-id-fresh", "0"), ctx)).toMatchObject({
				error: { code: "invalid_request", message: "id is invalid." },
			});
		});
	});

	it("rejects an envelope id that is not a non-empty string", async () => {
		await withContext(async (ctx, seededIds) => {
			const [first] = seededIds;
			for (const [index, envelopeId] of ["", 7, null].entries()) {
				expect(
					await dispatchDaemonRpc(getMany(`rpc-envelope-${index}`, String(first), envelopeId), ctx),
				).toMatchObject({
					error: { code: "invalid_request", message: "RPC request id is required." },
				});
			}
		});
	});
});
