import { chmodSync, copyFileSync } from "node:fs";
import { resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dirname, "../../..");
const source = resolve(repositoryRoot, "packages/cli/dist/hook-runtime.js");
// bundle と一緒に notice も置く。この 2 つの複製は npm package の `files` に入らず GitHub の
// source archive でだけ配布されるので、生成された notice が dist/ に留まっていると、bundle に
// 入っている commander の MIT 本文が複製に付いてこない（MIT は複製に許諾文の同梱を求める）。
const sourceNotice = resolve(repositoryRoot, "packages/cli/dist/THIRD_PARTY_NOTICES.hook-runtime.md");

for (const directory of [
	resolve(repositoryRoot, "plugins/claude/scripts"),
	resolve(repositoryRoot, "plugins/codex/scripts"),
]) {
	const target = resolve(directory, "hook-runtime.mjs");
	copyFileSync(source, target);
	chmodSync(target, 0o644);

	const targetNotice = resolve(directory, "THIRD_PARTY_NOTICES.hook-runtime.md");
	copyFileSync(sourceNotice, targetNotice);
	chmodSync(targetNotice, 0o644);
}
