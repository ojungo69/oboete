import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import net, { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { resolveStorageLayout } from "@codemem/core";
import { afterEach, describe, expect, it, vi } from "vitest";

const childProcessMocks = vi.hoisted(() => ({
	spawn: vi.fn(),
	spawnSync: vi.fn(() => ({
		pid: 0,
		output: [null, "", ""],
		stdout: "",
		stderr: "",
		status: 1,
		signal: null,
	})),
}));

const serverMocks = vi.hoisted(() => ({
	createViewerRpcCall: vi.fn(),
}));

vi.mock("node:child_process", async (importOriginal) => ({
	...(await importOriginal<typeof import("node:child_process")>()),
	spawn: childProcessMocks.spawn,
	spawnSync: childProcessMocks.spawnSync,
}));

vi.mock("@codemem/server", () => ({
	createViewerRpcCall: serverMocks.createViewerRpcCall,
}));

import { resolveDataDirOpt } from "../shared-options.js";
import {
	buildForegroundRunnerArgs,
	extractViewerPid,
	findTrustedSystemCommand,
	isLikelyViewerCommand,
	isLocalHost,
	isLoopbackOnlyHost,
	isSqliteVecLoadFailure,
	pickViewerPidCandidate,
	respondsLikeCodememViewer,
	serveCommand,
	sqliteVecFailureDiagnostics,
	terminateTrustedViewerPid,
} from "./serve.js";
import {
	resolveLegacyServeInvocation,
	resolveServeInvocation,
	resolveStartServeInvocation,
	resolveStopRestartInvocation,
} from "./serve-invocation.js";

describe("serve command option resolution", () => {
	afterEach(() => {
		vi.restoreAllMocks();
		vi.unstubAllGlobals();
		childProcessMocks.spawn.mockClear();
		childProcessMocks.spawnSync.mockClear();
		serverMocks.createViewerRpcCall.mockReset();
	});

	it("treats bare serve as a foreground start", async () => {
		const resolved = resolveLegacyServeInvocation({ host: "127.0.0.1", port: "38888" });
		expect(resolved).toEqual({
			mode: "start",
			dbPath: null,
			configPath: null,
			host: "127.0.0.1",
			port: 38888,
			background: false,
		});

		const port = await new Promise<number>((resolve, reject) => {
			const server = createServer();
			server.once("error", reject);
			server.listen(0, "127.0.0.1", () => {
				const address = server.address();
				if (!address || typeof address === "string") {
					server.close();
					reject(new Error("failed to reserve a loopback port"));
					return;
				}
				server.close((error) => (error ? reject(error) : resolve(address.port)));
			});
		});
		const root = mkdtempSync(join(tmpdir(), "codemem-serve-command-"));
		const dbPath = join(root, "isolated.sqlite");
		const originalExitCode = process.exitCode;
		const originalDataDir = process.env.CODEMEM_DATA_DIR;
		const originalDb = process.env.CODEMEM_DB;
		const originalConfig = process.env.CODEMEM_CONFIG;
		const output = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		try {
			delete process.env.CODEMEM_DATA_DIR;
			delete process.env.CODEMEM_DB;
			const siblingDbPath = join(root, "sibling.sqlite");
			expect(resolveDataDirOpt()).toBe(join(homedir(), ".codemem"));
			const customDataDir = resolveDataDirOpt({ dbPath });
			const siblingDataDir = resolveDataDirOpt({ dbPath: siblingDbPath });
			expect(dirname(customDataDir)).toBe(join(homedir(), ".codemem", "runtimes"));
			expect(basename(customDataDir)).toMatch(/^[a-f0-9]{32}$/);
			expect(resolveDataDirOpt({ dbPath })).toBe(customDataDir);
			expect(customDataDir).not.toBe(siblingDataDir);
			expect(resolveDataDirOpt({ dbPath: "~/custom.sqlite" })).toBe(
				resolveDataDirOpt({ dbPath: join(homedir(), "custom.sqlite") }),
			);
			const longDbPath = join(root, "x".repeat(180), "deeply-nested.sqlite");
			expect(
				Buffer.byteLength(
					resolveStorageLayout(resolveDataDirOpt({ dbPath: longDbPath })).socketPath,
				),
			).toBeLessThan(108);
			expect(resolveDataDirOpt({ dbPath: join(homedir(), ".codemem", "mem.sqlite") })).toBe(
				join(homedir(), ".codemem"),
			);
			const explicitDataDir = join(root, "explicit-data");
			process.env.CODEMEM_DATA_DIR = explicitDataDir;
			process.env.CODEMEM_DB = dbPath;
			expect(resolveDataDirOpt({ dbPath: siblingDbPath })).toBe(explicitDataDir);
			delete process.env.CODEMEM_DATA_DIR;
			delete process.env.CODEMEM_DB;

			process.exitCode = 0;
			await serveCommand.parseAsync(
				["stop", "--host", "127.0.0.1", "--port", String(port), "--db-path", dbPath],
				{ from: "user" },
			);
			expect(process.exitCode).toBe(0);
			expect(output.mock.calls.flat().join("")).toContain("No background viewer found");
			expect(childProcessMocks.spawn).not.toHaveBeenCalled();

			for (const action of ["start", "restart"]) {
				output.mockClear();
				process.exitCode = 0;
				await serveCommand.parseAsync(
					[action, "--host", "0.0.0.0", "--port", String(port), "--db-path", dbPath],
					{ from: "user" },
				);
				expect(process.exitCode, action).toBe(1);
				expect(output.mock.calls.flat().join(""), action).toContain(
					`Refusing to bind the viewer to non-loopback address 0.0.0.0:${port}.`,
				);
				expect(childProcessMocks.spawn, action).not.toHaveBeenCalled();
			}

			const newRuntime = join(root, "new-runtime");
			process.env.CODEMEM_DATA_DIR = newRuntime;
			const unref = vi.fn();
			childProcessMocks.spawn.mockReturnValue({ pid: 23_456, unref } as never);
			serverMocks.createViewerRpcCall.mockReturnValue(
				vi.fn(async (method: string) => {
					if (method === "POST /v1/viewer/auth/nonce") return { nonce: "n".repeat(43) };
					throw new Error(`unexpected ${method}`);
				}),
			);
			let connectionAttempt = 0;
			const connectionSpy = vi.spyOn(net, "createConnection").mockImplementation((() => {
				const socket = new net.Socket();
				queueMicrotask(() => {
					connectionAttempt += 1;
					if (connectionAttempt === 1) socket.emit("error", new Error("not listening"));
					else socket.emit("connect");
				});
				return socket;
			}) as typeof net.createConnection);
			output.mockClear();
			process.exitCode = 0;
			await serveCommand.parseAsync(
				["start", "--host", "127.0.0.1", "--port", String(port), "--db-path", dbPath],
				{ from: "user" },
			);
			expect(process.exitCode).toBe(0);
			expect(unref).toHaveBeenCalledOnce();
			expect(existsSync(newRuntime)).toBe(false);
			delete process.env.CODEMEM_DATA_DIR;

			output.mockClear();
			process.exitCode = 0;
			await serveCommand.parseAsync(["invalid-action"], { from: "user" });
			expect(process.exitCode).toBe(1);
			expect(output.mock.calls.flat().join("")).toContain("Unknown serve action: invalid-action");

			connectionSpy.mockImplementation((() => {
				const socket = new net.Socket();
				queueMicrotask(() => socket.emit("connect"));
				return socket;
			}) as typeof net.createConnection);
			const healthy = {
				service: "codemem-viewer",
				version: "0.0.0-test",
				pid: 12345,
				uptime_ms: 1_000,
				ready: true,
				database: { reachable: true },
			};
			const challengeNonce = "c".repeat(43);
			const freshNonce = "f".repeat(43);
			const session = "v1.signed-session.999.ZGFlbW9u.signature";
			mkdirSync(customDataDir, { recursive: true });
			writeFileSync(
				join(customDataDir, "viewer.pid"),
				JSON.stringify({ pid: 12_345, host: "127.0.0.1", port }),
			);
			const runExistingViewer = async (
				options: {
					health?: unknown;
					nonces?: unknown[];
					nonceErrorAt?: number;
					exchangeStatus?: number;
					exchangeBody?: unknown;
					loggedOut?: unknown;
				} = {},
			) => {
				const events: string[] = [];
				const nonces = [...(options.nonces ?? [challengeNonce, freshNonce])];
				const rpc = vi.fn(async (method: string, body: Record<string, unknown> = {}) => {
					if (method === "POST /v1/viewer/auth/nonce") {
						events.push("nonce");
						if (events.filter((event) => event === "nonce").length === options.nonceErrorAt) {
							throw new Error("nonce RPC failed");
						}
						return { nonce: nonces.shift() };
					}
					if (method === "POST /v1/viewer/auth/logout") {
						events.push(`logout:${String(body.session)}`);
						return { loggedOut: options.loggedOut ?? true };
					}
					throw new Error(`unexpected ${method}`);
				});
				serverMocks.createViewerRpcCall.mockReturnValue(rpc);
				const fetchMock = vi.fn<typeof fetch>(async (input, init) => {
					const pathname = new URL(String(input)).pathname;
					if (pathname === "/api/health") {
						events.push("health");
						return new Response(JSON.stringify(options.health ?? healthy), {
							status: 200,
							headers: { "content-type": "application/json" },
						});
					}
					if (pathname === "/api/auth/exchange") {
						const payload = JSON.parse(String(init?.body)) as { nonce?: unknown };
						expect(new Headers(init?.headers).get("Origin")).toBe(`http://127.0.0.1:${port}`);
						events.push(`exchange:${String(payload.nonce)}`);
						return new Response(JSON.stringify(options.exchangeBody ?? { session }), {
							status: options.exchangeStatus ?? 200,
							headers: { "content-type": "application/json" },
						});
					}
					throw new Error(`unexpected viewer request ${pathname}`);
				});
				vi.stubGlobal("fetch", fetchMock);
				output.mockClear();
				childProcessMocks.spawn.mockClear();
				process.exitCode = 0;
				await serveCommand.parseAsync(
					["start", "--host", "127.0.0.1", "--port", String(port), "--db-path", dbPath],
					{ from: "user" },
				);
				return { events, text: output.mock.calls.flat().join("") };
			};

			const reused = await runExistingViewer();
			expect(process.exitCode).toBe(0);
			expect(reused.events).toEqual([
				"health",
				"nonce",
				`exchange:${challengeNonce}`,
				`logout:${session}`,
				"nonce",
			]);
			expect(reused.text).toContain(
				`Viewer already running at http://127.0.0.1:${port}/#auth=${freshNonce}`,
			);
			expect(serverMocks.createViewerRpcCall).toHaveBeenLastCalledWith({
				socketPath: resolveStorageLayout(customDataDir).socketPath,
			});
			expect(childProcessMocks.spawn).not.toHaveBeenCalled();

			for (const failure of [
				{
					label: "wrong service",
					options: { health: { ...healthy, service: "other-service" } },
				},
				{
					label: "different daemon",
					options: { exchangeStatus: 401, exchangeBody: { error: "invalid or expired nonce" } },
				},
				{ label: "nonce RPC failure", options: { nonceErrorAt: 1 } },
				{ label: "malformed challenge nonce", options: { nonces: ["short"] } },
				{ label: "malformed session", options: { exchangeBody: { session: "bad session" } } },
				{ label: "logout refused", options: { loggedOut: false } },
				{
					label: "malformed fresh nonce",
					options: { nonces: [challengeNonce, "short"] },
				},
			]) {
				const rejected = await runExistingViewer(failure.options);
				expect(process.exitCode, failure.label).toBe(1);
				expect(rejected.text, failure.label).not.toContain("/#auth=");
				expect(childProcessMocks.spawn, failure.label).not.toHaveBeenCalled();
			}

			const viewerPid = 45_678;
			const stopSignals: string[] = [];
			let viewerRunning = true;
			vi.spyOn(process, "kill").mockImplementation((pid, signal) => {
				expect(pid).toBe(viewerPid);
				if (signal === 0) {
					if (!viewerRunning) throw new Error("not running");
					return true;
				}
				stopSignals.push(String(signal));
				viewerRunning = false;
				return true;
			});
			const runtimeRpc = vi.fn(async (method: string) => {
				if (method === "POST /v1/viewer/auth/nonce") return { nonce: "b".repeat(43) };
				throw new Error(`unexpected ${method}`);
			});
			serverMocks.createViewerRpcCall.mockReturnValue(runtimeRpc);
			const runtimeFetch = vi.fn<typeof fetch>(async (input) => {
				const pathname = new URL(String(input)).pathname;
				if (pathname === "/api/health") {
					return new Response(JSON.stringify(healthy), {
						status: 200,
						headers: { "content-type": "application/json" },
					});
				}
				if (pathname === "/api/auth/exchange") {
					return new Response(JSON.stringify({ error: "invalid or expired nonce" }), {
						status: 401,
						headers: { "content-type": "application/json" },
					});
				}
				if (pathname === "/api/stats") {
					return new Response(JSON.stringify({ viewer_pid: viewerPid }), {
						status: 200,
						headers: { "content-type": "application/json" },
					});
				}
				throw new Error(`unexpected viewer request ${pathname}`);
			});
			vi.stubGlobal("fetch", runtimeFetch);
			mkdirSync(siblingDataDir, { recursive: true });
			writeFileSync(
				join(siblingDataDir, "viewer.pid"),
				JSON.stringify({ pid: viewerPid, host: "127.0.0.1", port }),
			);
			output.mockClear();
			process.exitCode = 0;
			await serveCommand.parseAsync(
				["stop", "--host", "127.0.0.1", "--port", String(port), "--db-path", siblingDbPath],
				{ from: "user" },
			);
			expect(stopSignals).toEqual([]);
			expect(output.mock.calls.flat().join("")).toContain("No background viewer found");
			expect(runtimeRpc.mock.calls.map((call) => call[0])).toEqual(["POST /v1/viewer/auth/nonce"]);

			writeFileSync(
				join(customDataDir, "viewer.pid"),
				JSON.stringify({ pid: viewerPid, host: "::1", port }),
			);
			runtimeRpc.mockClear();
			runtimeFetch.mockClear();
			output.mockClear();
			process.exitCode = 0;
			await serveCommand.parseAsync(
				["start", "--host", "127.0.0.1", "--port", String(port), "--db-path", dbPath],
				{ from: "user" },
			);
			expect(output.mock.calls.flat().join("")).toContain(`already managed by viewer ::1:${port}`);
			expect(runtimeRpc).not.toHaveBeenCalled();
			expect(
				runtimeFetch.mock.calls.some(
					(call) => new URL(String(call[0])).pathname === "/api/auth/exchange",
				),
			).toBe(false);
		} finally {
			process.exitCode = originalExitCode;
			if (originalDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
			else process.env.CODEMEM_DATA_DIR = originalDataDir;
			if (originalDb === undefined) delete process.env.CODEMEM_DB;
			else process.env.CODEMEM_DB = originalDb;
			if (originalConfig === undefined) delete process.env.CODEMEM_CONFIG;
			else process.env.CODEMEM_CONFIG = originalConfig;
			output.mockRestore();
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("treats serve --background as a background start", () => {
		const resolved = resolveLegacyServeInvocation({
			host: "127.0.0.1",
			port: "38888",
			background: true,
		});
		expect(resolved.mode).toBe("start");
		expect(resolved.background).toBe(true);
	});

	it("maps serve --stop to stop mode", () => {
		const resolved = resolveLegacyServeInvocation({
			host: "127.0.0.1",
			port: "38888",
			stop: true,
		});
		expect(resolved.mode).toBe("stop");
		expect(resolved.background).toBe(false);
	});

	it("maps serve --restart to restart mode", () => {
		const resolved = resolveLegacyServeInvocation({
			host: "127.0.0.1",
			port: "38888",
			restart: true,
		});
		expect(resolved.mode).toBe("restart");
		expect(resolved.background).toBe(true);
	});

	it("rejects conflicting legacy stop and restart flags", () => {
		expect(() =>
			resolveLegacyServeInvocation({
				host: "127.0.0.1",
				port: "38888",
				stop: true,
				restart: true,
			}),
		).toThrow("Use only one of --stop or --restart");
	});

	it("maps serve stop to stop mode", () => {
		const resolved = resolveStopRestartInvocation("stop", {
			host: "127.0.0.1",
			port: "38888",
		});
		expect(resolved.mode).toBe("stop");
		expect(resolved.background).toBe(false);
	});

	it("maps serve restart to restart mode", () => {
		const resolved = resolveStopRestartInvocation("restart", {
			host: "127.0.0.1",
			port: "38888",
		});
		expect(resolved.mode).toBe("restart");
		expect(resolved.background).toBe(true);
	});

	it("defaults serve start to background mode", () => {
		const resolved = resolveStartServeInvocation({ host: "127.0.0.1", port: "38888" });
		expect(resolved.mode).toBe("start");
		expect(resolved.background).toBe(true);
	});

	it("supports serve start --foreground", () => {
		const resolved = resolveStartServeInvocation({
			host: "127.0.0.1",
			port: "38888",
			foreground: true,
		});
		expect(resolved.mode).toBe("start");
		expect(resolved.background).toBe(false);
	});

	it("supports serve start through the shared action resolver", () => {
		const resolved = resolveServeInvocation("start", {
			host: "127.0.0.1",
			port: "38888",
			foreground: true,
			config: "/tmp/workspace-config.json",
		});
		expect(resolved.mode).toBe("start");
		expect(resolved.background).toBe(false);
		expect(resolved.configPath).toBe("/tmp/workspace-config.json");
	});

	it("builds background child args from the current runner", () => {
		const args = buildForegroundRunnerArgs(
			"/repo/packages/cli/src/index.ts",
			{
				mode: "start",
				dbPath: "/tmp/test.sqlite",
				configPath: null,
				host: "127.0.0.1",
				port: 38991,
				background: true,
			},
			["--conditions", "source"],
		);
		expect(args).toEqual([
			"--conditions",
			"source",
			"/repo/packages/cli/src/index.ts",
			"serve",
			"start",
			"--foreground",
			"--host",
			"127.0.0.1",
			"--port",
			"38991",
			"--db-path",
			"/tmp/test.sqlite",
		]);
	});

	it("detects sqlite-vec load errors for viewer startup fallback", () => {
		expect(isSqliteVecLoadFailure(new Error("sqlite-vec loaded but version check failed"))).toBe(
			true,
		);
		expect(isSqliteVecLoadFailure(new Error("no such function: vec_version"))).toBe(true);
		expect(isSqliteVecLoadFailure(new Error("database is locked"))).toBe(false);
	});

	it("formats sqlite-vec diagnostics with runtime context", () => {
		const lines = sqliteVecFailureDiagnostics(new Error("vec0 load failed"), "/tmp/mem.sqlite");
		expect(lines.some((line) => line.startsWith("db=/tmp/mem.sqlite"))).toBe(true);
		expect(lines.some((line) => line.startsWith("node="))).toBe(true);
		expect(lines.some((line) => line.startsWith("exec="))).toBe(true);
		expect(lines.some((line) => line.startsWith("error=vec0 load failed"))).toBe(true);
	});

	it("extracts viewer_pid from stats payload", () => {
		expect(extractViewerPid({ viewer_pid: 12345 })).toBe(12345);
		expect(extractViewerPid({ pid: 54321 })).toBeNull();
		expect(extractViewerPid({ viewer_pid: -1 })).toBeNull();
		expect(extractViewerPid({ viewer_pid: "12345" })).toBeNull();
		expect(extractViewerPid({})).toBeNull();
	});

	it("uses degraded health for liveness without treating health pid as a stop candidate", async () => {
		const timeoutSpy = vi
			.spyOn(AbortSignal, "timeout")
			.mockReturnValue(new AbortController().signal);
		const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
			new Response(
				JSON.stringify({
					service: "codemem-viewer",
					pid: 54321,
					ready: false,
					database: { reachable: false },
				}),
				{ status: 200, headers: { "content-type": "application/json" } },
			),
		);

		await expect(
			respondsLikeCodememViewer({ pid: 12345, host: "127.0.0.1", port: 38_888 }, fetchMock),
		).resolves.toBe(true);
		expect(fetchMock).toHaveBeenCalledOnce();
		expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:38888/api/health");
		expect(timeoutSpy).toHaveBeenCalledWith(1000);
		expect(extractViewerPid({ pid: 54321 })).toBeNull();
	});

	it("accepts an old viewer through the 404 stats fallback", async () => {
		const fetchMock = vi
			.fn<typeof fetch>()
			.mockResolvedValueOnce(new Response(null, { status: 404 }))
			.mockResolvedValueOnce(
				new Response(JSON.stringify({ viewer_pid: 12345 }), {
					status: 200,
					headers: { "content-type": "application/json" },
				}),
			);

		await expect(
			respondsLikeCodememViewer({ pid: 12345, host: "127.0.0.1", port: 38_888 }, fetchMock),
		).resolves.toBe(true);
		expect(fetchMock.mock.calls.map((call) => call[0])).toEqual([
			"http://127.0.0.1:38888/api/health",
			"http://127.0.0.1:38888/api/stats",
		]);
	});

	it("selects pid candidate from stats and listener with mismatch protection", () => {
		expect(pickViewerPidCandidate(123, 123)).toBe(123);
		expect(pickViewerPidCandidate(null, 456)).toBe(456);
		expect(pickViewerPidCandidate(123, null)).toBe(123);
		expect(pickViewerPidCandidate(111, 222)).toBeNull();
	});

	it("recognizes local hosts for safe process control", () => {
		expect(isLocalHost("127.0.0.1")).toBe(true);
		expect(isLocalHost("localhost")).toBe(true);
		expect(isLocalHost("::1")).toBe(true);
		expect(isLocalHost("0.0.0.0")).toBe(true);
		expect(isLocalHost("example.com")).toBe(false);
	});

	it("only resolves process inspection tools from fixed absolute paths", () => {
		const dir = mkdtempSync(join(tmpdir(), "codemem-system-tool-"));
		const executable = join(dir, "ps");
		writeFileSync(executable, "");
		try {
			expect(findTrustedSystemCommand(["ps", executable])).toBe(executable);
			expect(findTrustedSystemCommand(["ps"])).toBeNull();
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	it("distinguishes loopback-only viewer binds from network-exposed binds", () => {
		expect(isLoopbackOnlyHost("127.0.0.1")).toBe(true);
		expect(isLoopbackOnlyHost("127.0.0.2")).toBe(true);
		expect(isLoopbackOnlyHost("127.1")).toBe(true);
		expect(isLoopbackOnlyHost("localhost")).toBe(true);
		expect(isLoopbackOnlyHost("::1")).toBe(true);
		expect(isLoopbackOnlyHost("0:0:0:0:0:0:0:1")).toBe(true);
		expect(isLoopbackOnlyHost("0.0.0.0")).toBe(false);
		expect(isLoopbackOnlyHost("::")).toBe(false);
		expect(isLoopbackOnlyHost("example.com")).toBe(false);
	});

	it("matches likely codemem viewer command lines", () => {
		expect(
			isLikelyViewerCommand(
				"node /Users/adam/.local/share/mise/installs/node/24.14.0/bin/codemem serve start --foreground --host 127.0.0.1 --port 38888",
			),
		).toBe(true);
		expect(
			isLikelyViewerCommand("node /repo/packages/cli/dist/index.js serve start --foreground"),
		).toBe(true);
		expect(isLikelyViewerCommand("node /repo/packages/cli/src/index.ts serve start")).toBe(true);
		expect(isLikelyViewerCommand("node /usr/bin/python -m http.server 38888")).toBe(false);
	});

	it("escalates trusted viewer shutdown when graceful SIGTERM stalls", async () => {
		const signals: string[] = [];
		let running = true;
		const killSpy = vi.spyOn(process, "kill").mockImplementation((pid, signal) => {
			expect(pid).toBe(12345);
			if (signal === 0) {
				if (!running) throw new Error("not running");
				return true;
			}
			signals.push(String(signal));
			if (signal === "SIGKILL") running = false;
			return true;
		});

		await expect(terminateTrustedViewerPid(12345, { gracefulMs: 1, forceMs: 50 })).resolves.toBe(
			true,
		);

		expect(signals).toEqual(["SIGTERM", "SIGKILL"]);
		expect(killSpy).toHaveBeenCalled();
	});
});
