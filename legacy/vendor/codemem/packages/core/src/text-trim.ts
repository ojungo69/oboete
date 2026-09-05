/**
 * Linear-time edge trimming.
 *
 * `s.replace(/x+$/, "")` looks harmless but is quadratic in the length of a run of `x`:
 * the engine retries the greedy `x+` from every start position and each attempt walks to
 * the end before `$` fails. A 32k run of `/` costs ~530ms; 128k costs ~8s. These helpers
 * run once over the affected edge instead, and every call site takes text that arrives
 * from a transcript, a hook payload, an import file, or a model response.
 *
 * Leading trims of the same shape (`/^x+/`) are already linear — the anchor pins the start
 * position — so they stay as regexes at their call sites and have no helper here.
 *
 * Predicates receive whole code points, not UTF-16 code units, so `\p{P}` and friends keep
 * behaving the way the `u`-flagged regexes they replace did.
 */

function codePointBefore(value: string, end: number): string {
	const last = value.charCodeAt(end - 1);
	if (last >= 0xdc00 && last <= 0xdfff && end >= 2) {
		const first = value.charCodeAt(end - 2);
		if (first >= 0xd800 && first <= 0xdbff) return value.slice(end - 2, end);
	}
	return value[end - 1] as string;
}

/** Drop code points matching `drop` from the end of `value`. */
export function trimEndWhere(value: string, drop: (char: string) => boolean): string {
	let end = value.length;
	while (end > 0) {
		const char = codePointBefore(value, end);
		if (!drop(char)) break;
		end -= char.length;
	}
	return value.slice(0, end);
}

/** Build a `drop` predicate from a literal character set. */
export function isOneOf(chars: string): (char: string) => boolean {
	const set = new Set(chars);
	return (char) => set.has(char);
}

const WHITESPACE = /^\s$/;

/** The `\s` class — includes NBSP and the Unicode separators, not just ASCII blanks. */
export function isWhitespace(char: string): boolean {
	return WHITESPACE.test(char);
}

const SPACE_OR_PUNCTUATION = /^[\s\p{P}]$/u;

/** Whitespace or Unicode punctuation — the `[\s\p{P}]` class, one code point at a time. */
export function isSpaceOrPunctuation(char: string): boolean {
	return SPACE_OR_PUNCTUATION.test(char);
}
