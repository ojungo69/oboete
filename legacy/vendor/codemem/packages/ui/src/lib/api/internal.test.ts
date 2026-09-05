import { afterEach, describe, expect, it, vi } from "vitest";
import { bootstrapViewerSession, payloadError, viewerFetch } from "./internal";

function sessionStore() {
	const values = new Map<string, string>();
	return {
		getItem: vi.fn((key: string) => values.get(key) ?? null),
		setItem: vi.fn((key: string, value: string) => values.set(key, value)),
	};
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe("viewer browser auth bootstrap", () => {
	it("P1-T043-06-browser-url-privacy removes every nonce before network use", async () => {
		const nonce = "n".repeat(43);
		const session = "signed-session";
		const replaceState = vi.fn();
		const storage = sessionStore();
		const fetchImpl = vi.fn(
			async () =>
				new Response(JSON.stringify({ session }), {
					status: 200,
					headers: { "Content-Type": "application/json" },
				}),
		);
		const exchanged = bootstrapViewerSession({
			hash: `#auth=${nonce}`,
			pathname: "/",
			search: "",
			state: { tab: "feed" },
			replaceState,
			fetch: fetchImpl,
			sessionStorage: storage,
		});

		expect(replaceState).toHaveBeenCalledWith({ tab: "feed" }, "", "/");
		expect(replaceState.mock.invocationCallOrder[0]).toBeLessThan(
			fetchImpl.mock.invocationCallOrder[0] ?? Number.MAX_SAFE_INTEGER,
		);
		await exchanged;
		expect(fetchImpl).toHaveBeenCalledWith(
			"/api/auth/exchange",
			expect.objectContaining({ method: "POST", credentials: "omit" }),
		);
		expect(storage.setItem).toHaveBeenCalledWith("codemem.viewer.session", session);
		expect(fetchImpl.mock.invocationCallOrder[0]).toBeLessThan(
			storage.setItem.mock.invocationCallOrder[0] ?? Number.MAX_SAFE_INTEGER,
		);

		const invalidReplaceState = vi.fn();
		const invalidFetch = vi.fn<typeof fetch>();
		const invalidStorage = sessionStore();
		await expect(
			bootstrapViewerSession({
				hash: "#auth=invalid",
				pathname: "/viewer",
				search: "?tab=feed",
				state: null,
				replaceState: invalidReplaceState,
				fetch: invalidFetch,
				sessionStorage: invalidStorage,
			}),
		).rejects.toThrow("Invalid viewer login nonce");
		expect(invalidReplaceState).toHaveBeenCalledWith(null, "", "/viewer?tab=feed");
		expect(invalidFetch).not.toHaveBeenCalled();
		expect(invalidStorage.setItem).not.toHaveBeenCalled();
		expect(
			payloadError({ error: { code: "daemon_unavailable", message: "Daemon is not running." } }),
		).toBe("Daemon is not running.");

		const viewerFetchImpl = vi.fn(
			async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(null, { status: 200 }),
		);
		vi.stubGlobal("window", { sessionStorage: storage });
		vi.stubGlobal("fetch", viewerFetchImpl);

		await viewerFetch("/api/runtime", { headers: { "X-Test": "preserved" } });

		expect(viewerFetchImpl).toHaveBeenCalledOnce();
		const [, init] = viewerFetchImpl.mock.calls[0] ?? [];
		if (!init) throw new Error("expected viewer fetch options");
		const headers = new Headers(init.headers);
		expect(headers.get("Authorization")).toBe("Session signed-session");
		expect(headers.get("X-Test")).toBe("preserved");
		expect(init.credentials).toBe("omit");

		await expect(viewerFetch("https://example.invalid/api/runtime")).rejects.toThrow(
			"Viewer API URL must be relative",
		);
		expect(viewerFetchImpl).toHaveBeenCalledOnce();
	});

	it("keeps the rest of the fragment when the nonce is not the only hash parameter", async () => {
		const nonce = "n".repeat(43);
		const replaceState = vi.fn();
		const fetchImpl = vi.fn(
			async () =>
				new Response(JSON.stringify({ session: "signed-session" }), {
					status: 200,
					headers: { "Content-Type": "application/json" },
				}),
		);

		await bootstrapViewerSession({
			hash: `#tab=health&auth=${nonce}`,
			pathname: "/viewer",
			search: "?project=demo",
			state: null,
			replaceState,
			fetch: fetchImpl,
			sessionStorage: sessionStore(),
		});

		expect(replaceState).toHaveBeenCalledWith(null, "", "/viewer?project=demo#tab=health");
	});

	it("resolves without touching history when the hash carries no nonce", async () => {
		const replaceState = vi.fn();
		const fetchImpl = vi.fn<typeof fetch>();

		await bootstrapViewerSession({
			hash: "#tab=health",
			pathname: "/viewer",
			search: "",
			state: null,
			replaceState,
			fetch: fetchImpl,
			sessionStorage: sessionStore(),
		});

		expect(replaceState).not.toHaveBeenCalled();
		expect(fetchImpl).not.toHaveBeenCalled();
	});
});
