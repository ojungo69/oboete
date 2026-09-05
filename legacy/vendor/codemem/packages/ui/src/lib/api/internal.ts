/* Internal fetch helpers shared across the API domain modules. The
 * error shape is consistent across viewer endpoints (best-effort JSON
 * body with an `error` field, falling back to the raw body text), so
 * every per-domain module can rely on payloadError + readJsonPayload
 * instead of hand-rolling the same try/catch. */

export function payloadError(payload: unknown): string | undefined {
	if (!payload || typeof payload !== "object") return undefined;
	const maybeError = (payload as { error?: unknown }).error;
	if (typeof maybeError === "string") return maybeError;
	if (!maybeError || typeof maybeError !== "object") return undefined;
	const message = (maybeError as { message?: unknown }).message;
	return typeof message === "string" ? message : undefined;
}

type ViewerSessionBootstrap = {
	hash: string;
	pathname: string;
	search: string;
	state: unknown;
	replaceState: (data: unknown, unused: string, url?: string | URL | null) => void;
	fetch: typeof fetch;
	sessionStorage: Pick<Storage, "getItem" | "setItem">;
};

const VIEWER_SESSION_STORAGE_KEY = "codemem.viewer.session";
const VIEWER_SESSION_PATTERN = /^[A-Za-z0-9._-]{1,512}$/;

export function bootstrapViewerSession(
	options: ViewerSessionBootstrap = {
		hash: window.location.hash,
		pathname: window.location.pathname,
		search: window.location.search,
		state: window.history.state,
		replaceState: window.history.replaceState.bind(window.history),
		fetch,
		sessionStorage: window.sessionStorage,
	},
): Promise<void> {
	const fragment = new URLSearchParams(options.hash.replace(/^#/, ""));
	const nonce = fragment.get("auth");
	if (nonce === null) return Promise.resolve();
	fragment.delete("auth");
	const remaining = fragment.toString();
	const hashSuffix = remaining ? `#${remaining}` : "";
	options.replaceState(options.state, "", `${options.pathname}${options.search}${hashSuffix}`);
	if (!/^[A-Za-z0-9_-]{43}$/.test(nonce)) {
		return Promise.reject(new Error("Invalid viewer login nonce"));
	}
	return options
		.fetch("/api/auth/exchange", {
			method: "POST",
			credentials: "omit",
			cache: "no-store",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ nonce }),
		})
		.then(async (response) => {
			if (!response.ok) throw new Error("Viewer login failed");
			const payload: unknown = await response.json();
			if (
				!payload ||
				typeof payload !== "object" ||
				Array.isArray(payload) ||
				typeof (payload as { session?: unknown }).session !== "string" ||
				!VIEWER_SESSION_PATTERN.test((payload as { session: string }).session)
			) {
				throw new Error("Viewer login failed");
			}
			options.sessionStorage.setItem(
				VIEWER_SESSION_STORAGE_KEY,
				(payload as { session: string }).session,
			);
		});
}

const viewerSessionReady =
	typeof window === "undefined" ? Promise.resolve() : bootstrapViewerSession();

export async function viewerFetch(input: string, init?: RequestInit): Promise<Response> {
	if (!input.startsWith("/api/")) {
		throw new Error("Viewer API URL must be relative");
	}
	await viewerSessionReady;
	const headers = new Headers();
	new Headers(init?.headers).forEach((value, key) => {
		headers.set(key, value);
	});
	const session =
		typeof window === "undefined"
			? null
			: window.sessionStorage.getItem(VIEWER_SESSION_STORAGE_KEY);
	if (session && VIEWER_SESSION_PATTERN.test(session)) {
		headers.set("Authorization", `Session ${session}`);
	}
	// The session-bearing request is restricted to the same-origin /api/ prefix above.
	return fetch(input, { ...init, credentials: "omit", headers }); // nosemgrep
}

export async function fetchJson<T = Record<string, unknown>>(url: string): Promise<T> {
	const resp = await viewerFetch(url);
	if (!resp.ok) throw new Error(`${url}: ${resp.status} ${resp.statusText}`);
	return resp.json() as Promise<T>;
}

export async function readJsonPayload<T = Record<string, unknown>>(
	resp: Response,
): Promise<{ text: string; payload: T }> {
	const text = await resp.text();
	try {
		return { text, payload: (text ? JSON.parse(text) : {}) as T };
	} catch {
		return { text, payload: {} as T };
	}
}

export function buildProjectParams(project: string, limit?: number, offset?: number): string {
	const params = new URLSearchParams();
	params.set("project", project || "");
	if (typeof limit === "number") params.set("limit", String(limit));
	if (typeof offset === "number") params.set("offset", String(offset));
	return params.toString();
}
