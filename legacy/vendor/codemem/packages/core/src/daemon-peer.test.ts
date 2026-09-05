import { chmodSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import * as core from "./index.js";

const created: Array<{ stop: () => Promise<void> }> = [];
const dirs: string[] = [];

afterEach(async () => {
	for (const handle of created.splice(0)) {
		try {
			await handle.stop();
		} catch {
			// cleanup
		}
	}
	for (const dir of dirs.splice(0)) {
		rmSync(dir, { recursive: true, force: true });
	}
});

describe("Phase 1 peer auth", () => {
	it("P1-T037-01-same-user-peer", async () => {
		const dir = mkdtempSync(join(tmpdir(), "codemem-peer-"));
		dirs.push(dir);
		const handle = await core.startDaemon({ dataDir: join(dir, "data") });
		created.push(handle);
		const ok = await core.callDaemonRpc(handle.socketPath, {
			id: "peer",
			method: "GET /v1/health",
			adapter_version: "1",
			native_cli_version: "1",
			normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
			local_api_version: core.LOCAL_API_VERSION,
			capability_hash: core.RPC_CAPABILITY_HASH,
		});
		expect(ok).toMatchObject({ result: { status: "ok" } });
		chmodSync(handle.socketPath, 0);
		try {
			const denied = await core.callDaemonRpc(handle.socketPath, {
				id: "peer-denied",
				method: "GET /v1/health",
				adapter_version: "1",
				native_cli_version: "1",
				normalized_schema_version: core.NORMALIZED_SCHEMA_VERSION,
				local_api_version: core.LOCAL_API_VERSION,
				capability_hash: core.RPC_CAPABILITY_HASH,
			});
			expect(denied).toMatchObject({
				error: { code: "peer_denied", retryable: false },
			});
		} finally {
			chmodSync(handle.socketPath, 0o600);
		}
		expect(core.mapPeerConnectError({ code: "EACCES", message: "denied" })).toMatchObject({
			error: { code: "peer_denied", retryable: false },
		});
		expect(core.mapPeerConnectError({ code: "ECONNREFUSED", message: "nope" })).toMatchObject({
			error: { code: "daemon_unavailable", retryable: true },
		});
		expect(statSync(handle.socketPath).mode & 0o077).toBe(0);
	});

	it("P1-T037-02-socket-permissions", async () => {
		const dir = mkdtempSync(join(tmpdir(), "codemem-perm-"));
		dirs.push(dir);
		const handle = await core.startDaemon({ dataDir: join(dir, "data") });
		created.push(handle);
		expect(statSync(handle.layout.controlDir).mode & 0o777).toBe(0o700);
		expect(statSync(handle.socketPath).mode & 0o777).toBe(0o600);
		expect(statSync(handle.identityPath).mode & 0o777).toBe(0o600);
		expect(statSync(handle.lockPath).mode & 0o777).toBe(0o600);
	});
});
