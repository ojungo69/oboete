/**
 * CORS and cross-origin protection middleware.
 *
 * Ports Python's reject_cross_origin() logic from codemem/viewer_http.py.
 * Every browser Origin must be loopback. Cookie-authenticated reads are
 * sensitive too, so safe methods do not receive a cross-origin exception.
 */

import { isLoopbackHost } from "@codemem/core";
import type { Context, Next } from "hono";
import { createMiddleware } from "hono/factory";

/**
 * Check whether an Origin header value is a valid loopback URL.
 * Mirrors Python's _is_allowed_loopback_origin_url().
 */
export function isLoopbackOrigin(origin: string): boolean {
	let url: URL;
	try {
		url = new URL(origin);
	} catch {
		return false;
	}
	if (url.protocol !== "http:" && url.protocol !== "https:") return false;
	if (url.username || url.password) return false;
	return isLoopbackHost(url.hostname);
}

export function isViewerOrigin(origin: string, requestUrl: string): boolean {
	if (!isLoopbackOrigin(origin)) return false;
	try {
		return new URL(origin).origin === new URL(requestUrl).origin;
	} catch {
		return false;
	}
}

const UNSAFE_METHODS = new Set(["POST", "DELETE", "PATCH", "PUT"]);

/**
 * Check whether a missing-Origin request looks like a cross-site browser
 * request. Matches Python's `_is_unsafe_missing_origin()`:
 *
 *   - Sec-Fetch-Site present and NOT same-origin/same-site/none → unsafe
 *   - Referer present and NOT loopback → unsafe
 *   - Otherwise → safe (CLI / programmatic caller, no browser context)
 */
function isUnsafeMissingOrigin(c: Context): boolean {
	const secFetchSite = (c.req.header("Sec-Fetch-Site") ?? "").trim().toLowerCase();
	if (secFetchSite && !["same-origin", "same-site", "none"].includes(secFetchSite)) {
		return true;
	}
	const referer = c.req.header("Referer");
	if (!referer) return false;
	return !isLoopbackOrigin(referer);
}

/**
 * Cross-origin protection middleware.
 *
 * Ports Python's `reject_cross_origin(missing_origin_policy="reject_if_unsafe")`:
 *
 * - Any supplied Origin must exactly match the viewer scheme, host, and port.
 * - POST/DELETE/PATCH/PUT without Origin:
 *   - No Origin + no suspicious browser signals → allowed (CLI callers)
 *   - No Origin + suspicious Sec-Fetch-Site/Referer → rejected 403
 *
 * For same-origin requests (no Origin header) on safe methods, no
 * Access-Control-Allow-Origin is set — the browser doesn't need it.
 * For valid loopback origins, ACAO is echoed back.
 */
export function originGuard() {
	return createMiddleware(async (c: Context, next: Next) => {
		const origin = c.req.header("Origin");
		const method = c.req.method;

		if (origin && !isViewerOrigin(origin, c.req.url)) {
			return c.json({ error: "forbidden" }, 403);
		}
		if (!origin && UNSAFE_METHODS.has(method) && isUnsafeMissingOrigin(c)) {
			return c.json({ error: "forbidden" }, 403);
		}
		if (origin) {
			c.header("Access-Control-Allow-Origin", origin);
			c.header("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
			c.header("Access-Control-Allow-Headers", "Authorization, Content-Type");
		}

		await next();
	});
}

/**
 * Handle OPTIONS preflight requests.
 * Returns 204 with appropriate CORS headers for loopback origins.
 */
export function preflightHandler() {
	return createMiddleware(async (c: Context, next: Next) => {
		if (c.req.method !== "OPTIONS") {
			await next();
			return;
		}
		const origin = c.req.header("Origin");
		if (origin && isViewerOrigin(origin, c.req.url)) {
			c.header("Access-Control-Allow-Origin", origin);
			c.header("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
			c.header("Access-Control-Allow-Headers", "Authorization, Content-Type");
			c.header("Access-Control-Max-Age", "86400");
			return c.body(null, 204);
		}
		return c.json({ error: "forbidden" }, 403);
	});
}

export function securityHeaders() {
	return createMiddleware(async (c: Context, next: Next) => {
		c.header("Referrer-Policy", "no-referrer");
		c.header("X-Content-Type-Options", "nosniff");
		c.header("X-Frame-Options", "DENY");
		c.header(
			"Content-Security-Policy",
			"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
		);
		await next();
	});
}
