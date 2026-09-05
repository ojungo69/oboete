/** @codemem/server — authenticated read-only viewer API + SPA host. */

import { randomUUID } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parseStrictInteger, VERSION, VIEWER_SERVICE_DISCRIMINATOR } from "@codemem/core";
import { serveStatic } from "@hono/node-server/serve-static";
import type { Context } from "hono";
import { Hono } from "hono";
import { bodyLimit } from "hono/body-limit";
import { isViewerOrigin, originGuard, preflightHandler, securityHeaders } from "./middleware.js";
import { createViewerRpcCall, type ViewerRpcCall, ViewerRpcError } from "./rpc-client.js";

export type { ViewerRpcCall } from "./rpc-client.js";
export { createViewerRpcCall } from "./rpc-client.js";
export { VERSION };

const TOKEN_PATTERN = /^[A-Za-z0-9_-]{43}$/;
const SESSION_PATTERN = /^[A-Za-z0-9._-]{1,512}$/;
const MAX_AUTH_BODY_BYTES = 1024;

const VIEW_ROUTES = new Map<string, string>([
	["/api/sessions", "sessions"],
	["/api/projects", "projects"],
	["/api/observations", "observations"],
	["/api/summaries", "summaries"],
	["/api/session", "session"],
	["/api/memory", "memory"],
	["/api/artifacts", "artifacts"],
	["/api/raw-events", "raw-events"],
	["/api/raw-events/status", "raw-events-status"],
	["/api/stats", "stats"],
	["/api/runtime", "runtime"],
	["/api/usage", "usage"],
	["/api/observer-status", "observer-status"],
	["/api/config", "config"],
]);

export interface AppOptions {
	rpc?: ViewerRpcCall;
}

function bearerValue(header: string | undefined): string | null {
	if (!header) return null;
	const match = /^Bearer ([A-Za-z0-9_-]{43})$/.exec(header);
	return match?.[1] ?? null;
}

function sessionValue(header: string | undefined): string | null {
	if (!header) return null;
	const match = /^Session ([A-Za-z0-9._-]{1,512})$/.exec(header);
	return match?.[1] ?? null;
}

function rpcFailure(c: Context, error: unknown) {
	const rpcError =
		error instanceof ViewerRpcError
			? error
			: new ViewerRpcError("daemon_unavailable", "Daemon is not running.", true);
	const unavailable =
		rpcError.code === "daemon_unavailable" || rpcError.code === "deadline_exceeded";
	return c.json(
		{ error: { code: rpcError.code, message: rpcError.message } },
		unavailable ? 503 : 502,
	);
}

function viewRequestBody(c: Context, collection: string): Record<string, unknown> {
	const body: Record<string, unknown> = { collection };
	const project = c.req.query("project");
	const kind = c.req.query("kind");
	const scope = c.req.query("scope");
	const limit = parseStrictInteger(c.req.query("limit"));
	const offset = parseStrictInteger(c.req.query("offset"));
	const sessionId = parseStrictInteger(c.req.query("session_id"));
	if (project) body.project = project;
	if (kind) body.kind = kind;
	if (scope) body.scope = scope;
	if (limit !== null) body.limit = limit;
	if (offset !== null) body.offset = offset;
	if (sessionId !== null) body.sessionId = sessionId;
	return body;
}

async function relayView(c: Context, rpc: ViewerRpcCall, collection: string) {
	try {
		const result = await rpc("GET /v1/view", viewRequestBody(c, collection));
		const status = result.status;
		if (
			typeof status !== "number" ||
			!Number.isInteger(status) ||
			status < 200 ||
			status > 599 ||
			!("body" in result)
		) {
			throw new ViewerRpcError("invalid_response", "Daemon returned an invalid viewer response.");
		}
		return c.json(result.body, status as 200);
	} catch (error) {
		return rpcFailure(c, error);
	}
}

async function readBoundedJson(c: Context, maxBytes: number): Promise<unknown> {
	const declared = Number.parseInt(c.req.header("content-length") ?? "0", 10);
	if (!Number.isFinite(declared) || declared < 0 || declared > maxBytes) {
		throw new Error("payload too large");
	}
	const raw = await c.req.text();
	if (Buffer.byteLength(raw, "utf8") > maxBytes) throw new Error("payload too large");
	return JSON.parse(raw);
}

function packFilters(
	project: unknown,
	workingSetFiles?: unknown,
): Record<string, unknown> | undefined {
	const filters: Record<string, unknown> = {};
	if (typeof project === "string" && project.trim()) filters.project = project;
	if (Array.isArray(workingSetFiles)) filters.working_set_paths = workingSetFiles;
	return Object.keys(filters).length > 0 ? filters : undefined;
}

export function createApp(opts: AppOptions = {}) {
	const rpc = opts.rpc ?? createViewerRpcCall();
	const app = new Hono();

	app.use("*", securityHeaders());
	app.use("/api/*", preflightHandler());
	app.use("/api/*", originGuard());
	app.use("/api/*", async (c, next) => {
		c.header("Cache-Control", "no-store");
		await next();
	});

	app.use(
		"/api/auth/exchange",
		bodyLimit({
			maxSize: MAX_AUTH_BODY_BYTES,
			onError: (c) => c.json({ error: "invalid request" }, 413),
		}),
	);
	app.post("/api/auth/exchange", async (c) => {
		const origin = c.req.header("Origin");
		if (!origin || !isViewerOrigin(origin, c.req.url)) {
			return c.json({ error: "forbidden" }, 403);
		}
		let payload: unknown;
		try {
			payload = await readBoundedJson(c, MAX_AUTH_BODY_BYTES);
		} catch {
			return c.json({ error: "invalid request" }, 400);
		}
		if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
			return c.json({ error: "invalid request" }, 400);
		}
		const values = payload as Record<string, unknown>;
		if (
			Object.keys(values).length !== 1 ||
			typeof values.nonce !== "string" ||
			!TOKEN_PATTERN.test(values.nonce)
		) {
			return c.json({ error: "invalid request" }, 400);
		}
		try {
			const result = await rpc("POST /v1/viewer/auth/exchange", { nonce: values.nonce });
			const session = result.session as { cookie?: unknown } | null;
			if (!session || typeof session.cookie !== "string" || !SESSION_PATTERN.test(session.cookie)) {
				return c.json({ error: "invalid or expired nonce" }, 401);
			}
			c.header("Cache-Control", "no-store");
			return c.json({ session: session.cookie });
		} catch (error) {
			return rpcFailure(c, error);
		}
	});

	// Public liveness must not require clients to disclose the durable bearer
	// before they have verified which process owns the loopback port.
	app.get("/api/health", async (c) => {
		try {
			const health = await rpc("GET /v1/health");
			const ready = health.status === "ok";
			return c.json({
				service: VIEWER_SERVICE_DISCRIMINATOR,
				version: VERSION,
				pid: process.pid,
				uptime_ms: Math.max(0, Math.floor(process.uptime() * 1_000)),
				ready,
				database: { reachable: ready },
			});
		} catch (error) {
			return rpcFailure(c, error);
		}
	});

	app.use("/api/*", async (c, next) => {
		const authorization = c.req.header("Authorization");
		const bearer = bearerValue(authorization);
		const session = sessionValue(authorization);
		if (!bearer && !session) {
			c.header("WWW-Authenticate", "Bearer");
			return c.json({ error: { code: "unauthorized", message: "Authentication required." } }, 401);
		}
		try {
			const result = await rpc("POST /v1/viewer/auth/verify", {
				...(bearer ? { bearer } : {}),
				...(session ? { session } : {}),
			});
			if (result.authenticated !== true) {
				c.header("WWW-Authenticate", "Bearer");
				return c.json({ error: { code: "unauthorized", message: "Invalid credential." } }, 401);
			}
			await next();
		} catch (error) {
			return rpcFailure(c, error);
		}
	});

	app.post("/api/auth/logout", async (c) => {
		const session = sessionValue(c.req.header("Authorization"));
		try {
			if (session) await rpc("POST /v1/viewer/auth/logout", { session });
			c.header("Cache-Control", "no-store");
			return c.body(null, 204);
		} catch (error) {
			return rpcFailure(c, error);
		}
	});

	app.get("/api/memories", (c) => c.redirect(`/api/observations${new URL(c.req.url).search}`, 301));
	for (const [path, collection] of VIEW_ROUTES) {
		app.get(path, (c) => relayView(c, rpc, collection));
	}

	app.get("/api/pack", async (c) => {
		const context = c.req.query("context")?.trim() ?? "";
		if (!context) return c.json({ error: "context required" }, 400);
		const limit = parseStrictInteger(c.req.query("limit")) ?? 10;
		const tokenBudgetText = c.req.query("token_budget");
		const tokenBudget =
			tokenBudgetText === undefined ? undefined : parseStrictInteger(tokenBudgetText);
		if (tokenBudgetText !== undefined && tokenBudget === null) {
			return c.json({ error: "token_budget must be int" }, 400);
		}
		const filters = packFilters(c.req.query("project"));
		try {
			const result = await rpc("POST /v1/context/pack", {
				requestId: randomUUID(),
				context,
				limit,
				...(tokenBudget === undefined ? {} : { tokenBudget }),
				...(filters ? { filters } : {}),
			});
			return c.json(result.pack);
		} catch (error) {
			return rpcFailure(c, error);
		}
	});

	app.use(
		"/api/pack/trace",
		bodyLimit({
			maxSize: 32 * 1024,
			onError: (c) => c.json({ error: "invalid json body" }, 413),
		}),
	);
	app.post("/api/pack/trace", async (c) => {
		let payload: unknown;
		try {
			payload = await readBoundedJson(c, 32 * 1024);
		} catch {
			return c.json({ error: "invalid json body" }, 400);
		}
		if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
			return c.json({ error: "invalid json body" }, 400);
		}
		const body = payload as Record<string, unknown>;
		const context = typeof body.context === "string" ? body.context.trim() : "";
		if (!context) return c.json({ error: "context required" }, 400);
		if (
			body.working_set_files !== undefined &&
			(!Array.isArray(body.working_set_files) ||
				!body.working_set_files.every((value) => typeof value === "string"))
		) {
			return c.json({ error: "working_set_files must be an array of strings" }, 400);
		}
		const filters = packFilters(body.project, body.working_set_files);
		try {
			const result = await rpc("POST /v1/context/pack", {
				requestId: randomUUID(),
				context,
				...(body.limit === undefined ? {} : { limit: body.limit }),
				...(body.token_budget === undefined ? {} : { tokenBudget: body.token_budget }),
				...(filters ? { filters } : {}),
				trace: true,
			});
			return c.json(result.trace);
		} catch (error) {
			return rpcFailure(c, error);
		}
	});

	app.all("/api/*", (c) => c.json({ error: "not found" }, 404));

	const staticRoot =
		process.env.CODEMEM_VIEWER_STATIC_DIR ?? join(import.meta.dirname ?? ".", "../static");
	app.use("/assets/*", async (c, next) => {
		c.header("Cache-Control", "no-cache");
		await next();
	});
	app.use(
		"/assets/*",
		serveStatic({
			root: staticRoot,
			rewriteRequestPath: (path) => path.replace(/^\/assets/, ""),
			precompressed: true,
		}),
	);

	const indexPath = join(staticRoot, "index.html");
	if (!existsSync(indexPath)) {
		throw new Error(
			`Viewer assets missing at ${indexPath}. Run \`pnpm build\` from the repo root before starting the viewer.`,
		);
	}
	const indexHtml = readFileSync(indexPath, "utf8");
	app.get("*", (c) => {
		c.header("Cache-Control", "no-store");
		return c.html(indexHtml);
	});

	return app;
}
