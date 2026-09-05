import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readViewerBearerToken, startDaemon } from "@codemem/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createApp, type ViewerRpcCall } from "./index.js";
import { createViewerRpcCall, ViewerRpcError } from "./rpc-client.js";

const roots: string[] = [];

function mountedApp(rpc: ViewerRpcCall) {
	const staticDir = mkdtempSync(join(tmpdir(), "codemem-viewer-auth-http-"));
	roots.push(staticDir);
	writeFileSync(join(staticDir, "index.html"), "<!doctype html><title>viewer</title>");
	const previous = process.env.CODEMEM_VIEWER_STATIC_DIR;
	process.env.CODEMEM_VIEWER_STATIC_DIR = staticDir;
	try {
		return createApp({ rpc });
	} finally {
		if (previous === undefined) delete process.env.CODEMEM_VIEWER_STATIC_DIR;
		else process.env.CODEMEM_VIEWER_STATIC_DIR = previous;
	}
}

afterEach(() => {
	vi.restoreAllMocks();
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("viewer HTTP security boundary", () => {
	it("P1-T043-01-browser-auth-401 rejects missing and incorrect credentials", async () => {
		const rpc = vi.fn<ViewerRpcCall>(async (method, body) => {
			if (method === "GET /v1/health") return { status: "ok" };
			if (method === "POST /v1/viewer/auth/verify") {
				if (body?.bearer === "e".repeat(43)) {
					throw new ViewerRpcError("permission_denied", "Credential verification failed.");
				}
				return { authenticated: body?.session === "valid-session" };
			}
			if (method === "GET /v1/view") return { status: 200, body: { version: "test" } };
			throw new Error(`unexpected ${method}`);
		});
		const app = mountedApp(rpc);

		expect((await app.request("/api/runtime")).status).toBe(401);
		expect((await app.request("/api/health")).status).toBe(200);
		expect(
			(
				await app.request("/api/runtime", {
					headers: { Authorization: `Bearer ${"i".repeat(43)}` },
				})
			).status,
		).toBe(401);
		expect(
			(
				await app.request("/api/runtime", {
					headers: { Authorization: "Session invalid session" },
				})
			).status,
		).toBe(401);
		expect(
			(
				await app.request("/api/runtime", {
					headers: { Authorization: "Session rejected-session" },
				})
			).status,
		).toBe(401);
		const validSession = await app.request("/api/runtime", {
			headers: { Authorization: "Session valid-session" },
		});
		expect(validSession.status).toBe(200);
		expect(await validSession.json()).toEqual({ version: "test" });
		const rpcError = await app.request("/api/runtime", {
			headers: { Authorization: `Bearer ${"e".repeat(43)}` },
		});
		expect(rpcError.status).toBe(502);
		expect(await rpcError.json()).toEqual({
			error: { code: "permission_denied", message: "Credential verification failed." },
		});
	});

	it("P1-T043-02-origin-403 rejects a malicious Origin before credential exchange", async () => {
		const rpc = vi.fn<ViewerRpcCall>();
		const app = mountedApp(rpc);
		const response = await app.request("/api/runtime", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}`, Origin: "https://attacker.example" },
		});
		expect(response.status).toBe(403);
		expect(rpc).not.toHaveBeenCalled();
		const otherLoopbackPort = await app.request("http://127.0.0.1:3737/api/runtime", {
			headers: {
				Authorization: `Bearer ${"v".repeat(43)}`,
				Origin: "http://127.0.0.1:9999",
			},
		});
		expect(otherLoopbackPort.status).toBe(403);
		const missingOrigin = await app.request("http://127.0.0.1:3737/api/auth/logout", {
			method: "POST",
			headers: {
				Authorization: `Bearer ${"v".repeat(43)}`,
				"Sec-Fetch-Site": "cross-site",
			},
		});
		expect(missingOrigin.status).toBe(403);
		const allowedPreflight = await app.request("http://127.0.0.1:3737/api/runtime", {
			method: "OPTIONS",
			headers: { Origin: "http://127.0.0.1:3737" },
		});
		expect(allowedPreflight.status).toBe(204);
		expect(allowedPreflight.headers.get("access-control-allow-origin")).toBe(
			"http://127.0.0.1:3737",
		);
		expect(allowedPreflight.headers.get("access-control-max-age")).toBe("86400");
		for (const origin of [undefined, "not a URL"]) {
			const rejectedPreflight = await app.request("http://127.0.0.1:3737/api/runtime", {
				method: "OPTIONS",
				headers: origin ? { Origin: origin } : undefined,
			});
			expect(rejectedPreflight.status, origin).toBe(403);
		}
		for (const origin of [undefined, "https://attacker.example"]) {
			const exchange = await app.request("http://127.0.0.1:3737/api/auth/exchange", {
				method: "POST",
				headers: {
					"Content-Type": "application/json",
					...(origin ? { Origin: origin } : {}),
				},
				body: JSON.stringify({ nonce: "v".repeat(43) }),
			});
			expect(exchange.status).toBe(403);
		}
		expect(rpc).not.toHaveBeenCalled();
	});

	it("P1-T043-11-loopback-cookie-csp retires cookie auth for loopback sessions", async () => {
		const nonce = "n".repeat(43);
		const rpc = vi.fn<ViewerRpcCall>(async (method, body) => {
			if (method === "POST /v1/viewer/auth/exchange") {
				if (body?.nonce === "r".repeat(43)) {
					throw new ViewerRpcError("nonce_failed", "Nonce exchange failed.");
				}
				return body?.nonce === nonce
					? { session: { cookie: "signed-session", expiresAt: Date.now() + 10_000 } }
					: { session: null };
			}
			if (method === "POST /v1/viewer/auth/verify") {
				return {
					authenticated:
						body?.session === "signed-session" ||
						body?.session === "logout-failure" ||
						body?.bearer === nonce,
				};
			}
			if (method === "POST /v1/viewer/auth/logout") {
				if (body?.session === "logout-failure") {
					throw new ViewerRpcError("logout_failed", "Logout failed.");
				}
				if (body?.session === "signed-session") return { loggedOut: true };
			}
			if (method === "GET /v1/view") {
				return { status: 200, body: { version: "test" } };
			}
			throw new Error(`unexpected ${method}`);
		});
		const app = mountedApp(rpc);
		const exchangeHeaders = {
			"Content-Type": "application/json",
			Origin: "http://127.0.0.1:3737",
		};
		let pulls = 0;
		const oversizedBody = new ReadableStream<Uint8Array>({
			pull(controller) {
				pulls += 1;
				controller.enqueue(new Uint8Array(600));
				if (pulls === 8) controller.close();
			},
		});
		const oversized = await app.request(
			new Request("http://127.0.0.1:3737/api/auth/exchange", {
				method: "POST",
				headers: exchangeHeaders,
				body: oversizedBody,
				duplex: "half",
			} as RequestInit & { duplex: "half" }),
		);
		expect(oversized.status).toBe(413);
		expect(pulls).toBeLessThan(8);
		expect(rpc).not.toHaveBeenCalled();
		for (const body of [
			"null",
			"[]",
			"{}",
			JSON.stringify({ nonce: "short" }),
			JSON.stringify({ nonce, extra: true }),
			"{",
		]) {
			const invalid = await app.request("http://127.0.0.1:3737/api/auth/exchange", {
				method: "POST",
				headers: exchangeHeaders,
				body,
			});
			expect(invalid.status, body).toBe(400);
		}
		const declaredOversized = await app.request("http://127.0.0.1:3737/api/auth/exchange", {
			method: "POST",
			headers: {
				...exchangeHeaders,
				"Content-Length": "1025",
			},
			body: JSON.stringify({ nonce }),
		});
		expect(declaredOversized.status).toBe(413);
		expect(rpc).not.toHaveBeenCalled();
		const invalidLength = await app.request("http://127.0.0.1:3737/api/auth/exchange", {
			method: "POST",
			headers: {
				...exchangeHeaders,
				"Content-Length": "invalid",
			},
			body: JSON.stringify({ nonce }),
		});
		expect(invalidLength.status).toBe(400);
		const invalidExchange = await app.request("http://127.0.0.1:3737/api/auth/exchange", {
			method: "POST",
			headers: exchangeHeaders,
			body: JSON.stringify({ nonce: "x".repeat(43) }),
		});
		expect(invalidExchange.status).toBe(401);
		const failedExchange = await app.request("http://127.0.0.1:3737/api/auth/exchange", {
			method: "POST",
			headers: exchangeHeaders,
			body: JSON.stringify({ nonce: "r".repeat(43) }),
		});
		expect(failedExchange.status).toBe(502);
		expect(await failedExchange.json()).toEqual({
			error: { code: "nonce_failed", message: "Nonce exchange failed." },
		});
		for (const origin of ["http://127.0.0.2:3737", "http://[::1]:3737"]) {
			const response = await app.request(`${origin}/api/auth/exchange`, {
				method: "POST",
				headers: { ...exchangeHeaders, Origin: origin },
				body: JSON.stringify({ nonce }),
			});

			expect(response.status).toBe(200);
			expect(response.headers.get("set-cookie")).toBeNull();
			expect(await response.json()).toEqual({ session: "signed-session" });
			expect(response.headers.get("referrer-policy")).toBe("no-referrer");
			expect(response.headers.get("content-security-policy")).toContain("script-src 'self'");
			expect(response.headers.get("content-security-policy")).not.toContain("unpkg.com");
		}

		const sessionAuth = await app.request("http://127.0.0.1:3737/api/runtime", {
			headers: { Authorization: "Session signed-session" },
		});
		expect(sessionAuth.status).toBe(200);

		const cookieOnly = await app.request("http://127.0.0.1:3737/api/runtime", {
			headers: { Cookie: "codemem_session=signed-session" },
		});
		expect(cookieOnly.status).toBe(401);

		const logout = await app.request("http://127.0.0.1:3737/api/auth/logout", {
			method: "POST",
			headers: {
				Authorization: "Session signed-session",
				Origin: "http://127.0.0.1:3737",
			},
		});
		expect(logout.status).toBe(204);
		expect(rpc).toHaveBeenCalledWith("POST /v1/viewer/auth/logout", {
			session: "signed-session",
		});
		const logoutCalls = rpc.mock.calls.filter(
			([method]) => method === "POST /v1/viewer/auth/logout",
		).length;
		const bearerLogout = await app.request("http://127.0.0.1:3737/api/auth/logout", {
			method: "POST",
			headers: { Authorization: `Bearer ${nonce}`, Origin: "http://127.0.0.1:3737" },
		});
		expect(bearerLogout.status).toBe(204);
		expect(
			rpc.mock.calls.filter(([method]) => method === "POST /v1/viewer/auth/logout"),
		).toHaveLength(logoutCalls);
		const failedLogout = await app.request("http://127.0.0.1:3737/api/auth/logout", {
			method: "POST",
			headers: {
				Authorization: "Session logout-failure",
				Origin: "http://127.0.0.1:3737",
			},
		});
		expect(failedLogout.status).toBe(502);
		expect(await failedLogout.json()).toEqual({
			error: { code: "logout_failed", message: "Logout failed." },
		});
	});

	it("P1-T043-08-viewer-daemon-unavailable returns typed 503 without the daemon", async () => {
		const rpc = vi.fn<ViewerRpcCall>(async (method) => {
			if (method === "POST /v1/viewer/auth/verify") return { authenticated: true };
			if (method === "GET /v1/view") {
				return { status: 200, body: { version: "test" } };
			}
			throw new Error(`unexpected ${method}`);
		});
		const app = mountedApp(rpc);
		const ok = await app.request("/api/runtime", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}` },
		});
		expect(ok.status).toBe(200);
		expect(ok.headers.get("cache-control")).toBe("no-store");
		expect(await ok.json()).toEqual({ version: "test" });

		const unavailable = mountedApp(async () => {
			throw new Error("daemon unavailable");
		});
		const response = await unavailable.request("/api/runtime", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}` },
		});
		expect(response.status).toBe(503);
		expect(await response.json()).toEqual({
			error: { code: "daemon_unavailable", message: "Daemon is not running." },
		});

		const rejected = mountedApp(async (method) => {
			if (method === "POST /v1/viewer/auth/verify") return { authenticated: true };
			throw new ViewerRpcError("invalid_response", "Daemon response was invalid.");
		});
		const rejectedResponse = await rejected.request("/api/runtime", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}` },
		});
		expect(rejectedResponse.status).toBe(502);
		expect(await rejectedResponse.json()).toEqual({
			error: { code: "invalid_response", message: "Daemon response was invalid." },
		});
		const unhealthy = mountedApp(async () => {
			throw new ViewerRpcError("deadline_exceeded", "Health check timed out.", true);
		});
		const unhealthyResponse = await unhealthy.request("/api/health");
		expect(unhealthyResponse.status).toBe(503);
		expect(await unhealthyResponse.json()).toEqual({
			error: { code: "deadline_exceeded", message: "Health check timed out." },
		});
	});

	it("P1-T043-07-viewer-read-only exposes no legacy mutation routes", async () => {
		const rpc = vi.fn<ViewerRpcCall>(async (method, body) => {
			if (method === "POST /v1/viewer/auth/verify") return { authenticated: true };
			if (method === "GET /v1/view") {
				if (body?.collection === "sessions") return { status: 199, body: {} };
				return { status: 200, body: { items: [] } };
			}
			if (method === "POST /v1/context/pack") {
				if (body?.context === "boom") {
					throw new ViewerRpcError("pack_failed", "Pack request failed.");
				}
				return body?.trace === true
					? { trace: { version: 1, context: body.context } }
					: { pack: { pack_text: "viewer pack" } };
			}
			throw new Error(`unexpected ${method}`);
		});
		const app = mountedApp(rpc);
		const relay = await app.request(
			"http://127.0.0.1:3737/api/observations?project=demo&kind=decision&scope=team&limit=5&offset=2&session_id=7",
			{ headers: { Authorization: `Bearer ${"v".repeat(43)}` } },
		);
		expect(relay.status).toBe(200);
		expect(await relay.json()).toEqual({ items: [] });
		expect(rpc).toHaveBeenCalledWith("GET /v1/view", {
			collection: "observations",
			project: "demo",
			kind: "decision",
			scope: "team",
			limit: 5,
			offset: 2,
			sessionId: 7,
		});
		const invalidRelay = await app.request("http://127.0.0.1:3737/api/sessions", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}` },
		});
		expect(invalidRelay.status).toBe(502);
		expect(await invalidRelay.json()).toEqual({
			error: {
				code: "invalid_response",
				message: "Daemon returned an invalid viewer response.",
			},
		});
		const traceHeaders = {
			Authorization: `Bearer ${"v".repeat(43)}`,
			"Content-Type": "application/json",
			Origin: "http://127.0.0.1:3737",
		};
		for (const path of [
			"/api/config",
			"/api/memories/project",
			"/api/memories/forget",
			"/api/memories/visibility",
			"/api/raw-events",
			"/api/claude-hooks",
			"/api/codex-hooks",
			"/api/prompt-pack-ledger",
			"/api/pack",
		]) {
			const response = await app.request(`http://127.0.0.1:3737${path}`, {
				method: "POST",
				headers: traceHeaders,
				body: "{}",
			});
			expect(response.status, path).toBe(404);
		}
		const workingSetError = "working_set_files must be an array of strings";
		for (const [body, error] of [
			["null", "invalid json body"],
			["[]", "invalid json body"],
			["{}", "context required"],
			[JSON.stringify({ context: "   " }), "context required"],
			[JSON.stringify({ context: "test", working_set_files: "not-an-array" }), workingSetError],
			[
				JSON.stringify({ context: "test", working_set_files: ["src/index.ts", 1] }),
				workingSetError,
			],
			["{", "invalid json body"],
		]) {
			const invalidBody = await app.request("http://127.0.0.1:3737/api/pack/trace", {
				method: "POST",
				headers: traceHeaders,
				body,
			});
			expect(invalidBody.status, body).toBe(400);
			expect(await invalidBody.json()).toEqual({ error });
		}
		const oversizedTrace = await app.request("http://127.0.0.1:3737/api/pack/trace", {
			method: "POST",
			headers: {
				...traceHeaders,
				"Content-Length": String(32 * 1024 + 1),
			},
			body: JSON.stringify({ context: "test" }),
		});
		expect(oversizedTrace.status).toBe(413);
		expect(await oversizedTrace.json()).toEqual({ error: "invalid json body" });

		const missingPackContext = await app.request("http://127.0.0.1:3737/api/pack", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}` },
		});
		expect(missingPackContext.status).toBe(400);
		expect(await missingPackContext.json()).toEqual({ error: "context required" });
		const invalidBudget = await app.request(
			"http://127.0.0.1:3737/api/pack?context=test&token_budget=large",
			{ headers: { Authorization: `Bearer ${"v".repeat(43)}` } },
		);
		expect(invalidBudget.status).toBe(400);
		expect(await invalidBudget.json()).toEqual({ error: "token_budget must be int" });
		const contextOnly = await app.request("http://127.0.0.1:3737/api/pack?context=only", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}` },
		});
		expect(contextOnly.status).toBe(200);
		expect(rpc).toHaveBeenCalledWith("POST /v1/context/pack", {
			requestId: expect.any(String),
			context: "only",
			limit: 10,
		});
		const packed = await app.request(
			"http://127.0.0.1:3737/api/pack?context=continue&project=demo&limit=4&token_budget=50",
			{ headers: { Authorization: `Bearer ${"v".repeat(43)}` } },
		);
		expect(packed.status).toBe(200);
		expect(await packed.json()).toEqual({ pack_text: "viewer pack" });
		expect(rpc).toHaveBeenCalledWith("POST /v1/context/pack", {
			requestId: expect.any(String),
			context: "continue",
			limit: 4,
			tokenBudget: 50,
			filters: { project: "demo" },
		});
		const packFailure = await app.request("http://127.0.0.1:3737/api/pack?context=boom", {
			headers: { Authorization: `Bearer ${"v".repeat(43)}` },
		});
		expect(packFailure.status).toBe(502);
		expect(await packFailure.json()).toEqual({
			error: { code: "pack_failed", message: "Pack request failed." },
		});

		const contextOnlyTrace = await app.request("http://127.0.0.1:3737/api/pack/trace", {
			method: "POST",
			headers: traceHeaders,
			body: JSON.stringify({ context: "  trace only  ", project: "   " }),
		});
		expect(contextOnlyTrace.status).toBe(200);
		expect(await contextOnlyTrace.json()).toEqual({ version: 1, context: "trace only" });
		expect(rpc).toHaveBeenCalledWith("POST /v1/context/pack", {
			requestId: expect.any(String),
			context: "trace only",
			trace: true,
		});
		const traced = await app.request("http://127.0.0.1:3737/api/pack/trace", {
			method: "POST",
			headers: traceHeaders,
			body: JSON.stringify({
				context: "continue",
				project: "demo",
				working_set_files: ["src/index.ts"],
				limit: 3,
				token_budget: 40,
			}),
		});
		expect(traced.status).toBe(200);
		expect(await traced.json()).toEqual({ version: 1, context: "continue" });
		expect(rpc).toHaveBeenCalledWith("POST /v1/context/pack", {
			requestId: expect.any(String),
			context: "continue",
			limit: 3,
			tokenBudget: 40,
			filters: { project: "demo", working_set_paths: ["src/index.ts"] },
			trace: true,
		});
		const traceFailure = await app.request("http://127.0.0.1:3737/api/pack/trace", {
			method: "POST",
			headers: traceHeaders,
			body: JSON.stringify({ context: "boom" }),
		});
		expect(traceFailure.status).toBe(502);
		expect(await traceFailure.json()).toEqual({
			error: { code: "pack_failed", message: "Pack request failed." },
		});
	});

	it("P1-T043-13-daemon-restart-session invalidates an HTTP session on restart", async () => {
		const root = mkdtempSync(join(tmpdir(), "codemem-viewer-auth-e2e-"));
		roots.push(root);
		const dataDir = join(root, "data");
		let daemon = await startDaemon({ dataDir });
		const rpc = createViewerRpcCall({ socketPath: daemon.socketPath });
		const app = mountedApp(rpc);
		try {
			const bearer = readViewerBearerToken(daemon.layout.controlDir);
			expect(bearer).toMatch(/^[A-Za-z0-9_-]{43}$/);
			expect(
				(
					await app.request("/api/runtime", {
						headers: { Authorization: `Bearer ${bearer}` },
					})
				).status,
			).toBe(200);
			await expect(rpc("GET /v1/unknown" as never)).rejects.toMatchObject({
				name: "ViewerRpcError",
				code: "unknown_method",
				message: "Unknown RPC method: GET /v1/unknown",
				retryable: false,
			});

			const issued = await rpc("POST /v1/viewer/auth/nonce");
			const exchange = await app.request("http://127.0.0.1:3737/api/auth/exchange", {
				method: "POST",
				headers: { "Content-Type": "application/json", Origin: "http://127.0.0.1:3737" },
				body: JSON.stringify({ nonce: issued.nonce }),
			});
			const session = ((await exchange.json()) as { session?: unknown }).session;
			expect(session).toMatch(/^v1\./);
			expect(exchange.headers.get("set-cookie")).toBeNull();
			expect(
				(
					await app.request("/api/runtime", {
						headers: { Authorization: `Session ${String(session)}` },
					})
				).status,
			).toBe(200);

			await daemon.stop();
			await expect(rpc("GET /v1/health")).rejects.toMatchObject({
				name: "ViewerRpcError",
				code: "daemon_unavailable",
				message: "Daemon is not running.",
				retryable: true,
			});
			daemon = await startDaemon({ dataDir });
			expect(
				(
					await app.request("/api/runtime", {
						headers: { Authorization: `Session ${String(session)}` },
					})
				).status,
			).toBe(401);
		} finally {
			await daemon.stop();
		}
	});
});
