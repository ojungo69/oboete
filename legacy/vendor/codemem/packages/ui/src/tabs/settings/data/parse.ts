/* JSON parser for the observer HTTP headers object. */

export function parseObserverHeaders(raw: string): Record<string, string> {
	const text = raw.trim();
	if (!text) return {};
	const parsed = JSON.parse(text);
	if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
		throw new Error("observer headers must be a JSON object");
	}
	const headers: Record<string, string> = {};
	for (const [key, value] of Object.entries(parsed)) {
		if (typeof key !== "string" || !key.trim() || typeof value !== "string") {
			throw new Error("observer headers must map string keys to string values");
		}
		headers[key.trim()] = value;
	}
	return headers;
}
