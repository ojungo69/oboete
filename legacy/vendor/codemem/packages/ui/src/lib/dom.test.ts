import { describe, expect, it } from "vitest";
import { escapeHtml, escapeRegExp } from "./dom";

describe("dom string escaping", () => {
	it("escapes every HTML metacharacter", () => {
		expect(escapeHtml(`<a href="x">&'</a>`)).toBe(
			"&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;",
		);
	});

	it("escapes every regex metacharacter and leaves other characters alone", () => {
		expect(escapeRegExp("a.b*c(d)")).toBe(String.raw`a\.b\*c\(d\)`);
		expect(escapeRegExp("^$+?|[]{}\\")).toBe(String.raw`\^\$\+\?\|\[\]\{\}\\`);
		expect(escapeRegExp("plain text 42")).toBe("plain text 42");
	});
});
