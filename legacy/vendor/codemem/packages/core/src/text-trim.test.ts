import { describe, expect, it } from "vitest";
import { stripTrailingFence } from "./maintenance/ai-structured.js";
import { FILENAME_LOCATOR, IMPLEMENTATION_LOCATOR_PATTERNS } from "./memory-quality.js";
import { isOneOf, isSpaceOrPunctuation, isWhitespace, trimEndWhere } from "./text-trim.js";

const SLASH = isOneOf("/");

describe("trimEndWhere", () => {
	it("matches the regexes they replace", () => {
		// 置き換え元と同じ結果になることを、境界を含めて確かめる
		const cases = ["", "/", "//", "a/", "a//", "/a", "//a//", "a", "/a/b//", "  ", "a  "];
		for (const s of cases) {
			expect(trimEndWhere(s, SLASH)).toBe(s.replace(/\/+$/, ""));
			expect(trimEndWhere(s, isWhitespace)).toBe(s.replace(/\s+$/, ""));
		}
	});

	it("leaves the middle alone", () => {
		expect(trimEndWhere("--a--b--", isOneOf("-"))).toBe("--a--b");
		expect(trimEndWhere("---", isOneOf("-"))).toBe("");
		expect(trimEndWhere("", isOneOf("-"))).toBe("");
	});

	it("treats surrogate pairs as one code point, like the /u regexes", () => {
		// 星域面の句読点。code unit 単位で見ると U+D800 台の片割れになり判定を外す
		const aegeanWordSeparator = "\u{10100}";
		expect(isSpaceOrPunctuation(aegeanWordSeparator)).toBe(true);
		const s = `a${aegeanWordSeparator}${aegeanWordSeparator}`;
		expect(trimEndWhere(s, isSpaceOrPunctuation)).toBe("a");
		expect(trimEndWhere(s, isSpaceOrPunctuation)).toBe(s.replace(/[\s\p{P}]+$/u, ""));
		// 絵文字は \p{P} ではないので落とさない（元の正規表現と同じ）
		expect(trimEndWhere("a😀", isSpaceOrPunctuation)).toBe("a😀");
	});

	it("does not scale with the length of the trimmed run", () => {
		// `s.replace(/\/+$/, "")` はここで二次に走る（32k で ~530ms、128k で ~8s）
		const started = performance.now();
		const huge = `${"/".repeat(512 * 1024)}x`;
		expect(trimEndWhere(huge, SLASH)).toBe(huge);
		expect(trimEndWhere(`x${"/".repeat(512 * 1024)}`, SLASH)).toBe("x");
		expect(performance.now() - started).toBeLessThan(1000);
	});
});

describe("ReDoS を外した正規表現の等価性", () => {
	it("ファイル名検出は元の正規表現と同じ判定になる", () => {
		const before = /[\w.-]+\.(?:ts|tsx|js|jsx|mjs|cjs|json|md|sql|yaml|yml)\b/;
		// 出荷している定数そのものを見る。test 内に写すと本番を壊しても緑のままになる
		const after = FILENAME_LOCATOR;
		const cases = [
			"see packages/core/src/store.ts for details",
			"a.ts",
			".ts",
			"x..ts",
			"foo-bar.test.ts",
			"README.md and schema.sql",
			"no file here",
			"tsconfig",
			"a.tsx b.yml",
			"weird.ts.bak",
			"-.md",
			"_.json",
		];
		for (const s of cases) expect(after.test(s)).toBe(before.test(s));
		// 判定に使われている配列に載っていなければ、この等価性は本番に効いていない
		expect(IMPLEMENTATION_LOCATOR_PATTERNS).toContain(FILENAME_LOCATOR);
	});

	it("フェンス剥がしは元の正規表現と同じ結果になる", () => {
		for (const s of ['{"a":1}', '{"a":1}\n```', '{"a":1}```', "```", "  ```", "", "a```b"]) {
			expect(stripTrailingFence(s)).toBe(s.replace(/\s*```$/, ""));
		}
	});
});
