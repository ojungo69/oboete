import { createHash } from "node:crypto";
import { isSpaceOrPunctuation, trimEndWhere } from "./text-trim.js";

export function normalizeMemoryDedupTitle(title: string): string {
	// 先頭側の `/^[\s\p{P}]+/u` は anchor が開始位置を固定するので線形。末尾側だけが
	// 入力長に対して二次に走るため helper に置き換える（memory の title は外から来る）
	const base = title
		.toLowerCase()
		.replace(/\b(?:pr|pull\s+request|issue)\s*#?\d+\b/gi, " ")
		.replace(/^\s*#\d+\s*/g, " ")
		.replace(/^[\s\p{P}]+/u, "");
	return trimEndWhere(base, isSpaceOrPunctuation).replace(/\s+/g, " ").trim();
}

export function buildMemoryDedupKey(title: string): string | null {
	const normalized = normalizeMemoryDedupTitle(title);
	const fallback = title.toLowerCase().replace(/\s+/g, " ").trim();
	const keySource = normalized || fallback;
	if (!keySource) return null;
	return createHash("sha256").update(keySource).digest("hex");
}
