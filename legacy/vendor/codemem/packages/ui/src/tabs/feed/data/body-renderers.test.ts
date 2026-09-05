import { describe, expect, it } from "vitest";
import { isLabeledFact } from "./body-renderers.js";

/** 置き換え元。`.+?` と `.+` が開始位置ごとに走り直すため長い 1 行で二次に効く */
const ORIGINAL = /.+?:\s+.+/;

describe("isLabeledFact", () => {
	it("matches the regex it replaced", () => {
		const cases = [
			"label: value",
			"label:value",
			"label:  value",
			": value",
			"a:b: c",
			"a:\nb",
			"a:\n",
			"",
			":",
			"a: ",
			"one two: three four",
			// `.` は改行以外にも CR・行区切り・段落区切りを外す。文字クラスを `[^\n]` だけに
			// すると、この 4 件が元と食い違う（レビューで実測された差分そのもの）
			"summary\r: done",
			"summary: \rdone",
			"summary\u2028: done",
			"summary\u2029: done",
		];
		for (const s of cases) expect(isLabeledFact(s)).toBe(ORIGINAL.test(s));
	});

	it("does not scale with the length of the line", () => {
		// ORIGINAL はこの入力で二次に走る（`:` が無いので全開始位置を試す）
		const started = performance.now();
		expect(isLabeledFact("x".repeat(256 * 1024))).toBe(false);
		expect(performance.now() - started).toBeLessThan(1000);
	});
});
