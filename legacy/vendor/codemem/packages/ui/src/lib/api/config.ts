/* Read-only observer/config endpoints. */

import { fetchJson } from "./internal";

export async function loadObserverStatus(): Promise<unknown> {
	return fetchJson("/api/observer-status");
}

export async function loadConfig(): Promise<unknown> {
	return fetchJson("/api/config");
}
