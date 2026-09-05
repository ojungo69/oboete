#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const EXPECTED = {
	baseline: 4_037,
	retired: 2_376,
	additions: 206,
	final: 1_867,
	passed: 1_864,
	todo: 3,
	tokens: 86,
};

const reportArgument = process.argv[2];
if (!reportArgument) throw new Error("usage: node harness/phase1-test-set-compare.mjs <vitest-report.json>");

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const vendorRoot = resolve(repoRoot, "vendor/codemem");
const manifestLines = readFileSync(
	resolve(repoRoot, "evidence/phase1-test-retire-list.md"),
	"utf8",
).split("\n");
const baselineRows = readFileSync(
	resolve(repoRoot, "evidence/phase1-test-baseline-pre.txt"),
	"utf8",
)
	.split("\n")
	.filter((line) => line && !line.startsWith("#"))
	.map((line) => {
		const match = line.match(/^(.*) \[([^\]]+)\]$/);
		if (!match) throw new Error(`invalid baseline row: ${line}`);
		return { key: match[1], status: match[2] };
	});

const report = JSON.parse(readFileSync(resolve(reportArgument), "utf8"));
if (!Array.isArray(report.testResults)) throw new Error("invalid Vitest JSON: testResults is missing");

const finalRows = report.testResults.flatMap((file) => {
	if (typeof file.name !== "string" || !Array.isArray(file.assertionResults)) {
		throw new Error("invalid Vitest JSON: malformed test result");
	}
	const fileName = relative(vendorRoot, resolve(file.name)).split(sep).join("/");
	if (fileName === ".." || fileName.startsWith("../")) {
		throw new Error(`test file is outside vendor/codemem: ${file.name}`);
	}
	return file.assertionResults.map((test) => {
		if (!Array.isArray(test.ancestorTitles) || typeof test.title !== "string") {
			throw new Error(`invalid Vitest assertion in ${file.name}`);
		}
		return {
			key: `${fileName} > ${[...test.ancestorTitles, test.title].join(" > ")}`,
			status: test.status,
		};
	});
});

const count = (items) => {
	const counts = new Map();
	for (const item of items) counts.set(item, (counts.get(item) ?? 0) + 1);
	return counts;
};

const difference = (left, right) => {
	const remaining = count(right);
	return left.filter((item) => {
		const available = remaining.get(item) ?? 0;
		if (available === 0) return true;
		remaining.set(item, available - 1);
		return false;
	});
};

const section = (startHeading, endHeading) => {
	const start = manifestLines.findIndex((line) => line.startsWith(startHeading));
	if (start === -1) throw new Error(`manifest heading is missing: ${startHeading}`);
	const end = endHeading
		? manifestLines.findIndex((line, index) => index > start && line.startsWith(endHeading))
		: manifestLines.length;
	if (end === -1) throw new Error(`manifest heading is missing: ${endHeading}`);
	return manifestLines
		.slice(start + 1, end)
		.filter(
			(line) =>
				line.startsWith("packages/") ||
				line.startsWith("e2e/") ||
				line.startsWith("- packages/") ||
				line.startsWith("- e2e/"),
		)
		.map((line) => (line.startsWith("- ") ? line.slice(2) : line));
};

const exactAdditions = [
	...section("## A7 中に追加済み", "## Phase 1 追加予定"),
	...section("## T058 final post-only exact additions", "## T043 retired"),
];
const tokens = manifestLines.flatMap((line) => {
	const match = line.match(/^\| `(P1-[^`]+)`/);
	return match ? [match[1]] : [];
});
const retired = [
	...section("## T043 retired", "## T044 retired"),
	...section("## T044 retired", "## T045 retired"),
	...section("## T045 retired", "## T047 retired"),
	...section("## T047 retired", "## T048 retired"),
	...section("## T048 retired", "## Retired fully qualified names"),
	...section("## Retired fully qualified names", "## T058 additional baseline retired"),
	...section("## T058 additional baseline retired", undefined),
];

const baselineKeys = baselineRows.map((row) => row.key);
const finalKeys = finalRows.map((row) => row.key);
const actualRetired = difference(baselineKeys, finalKeys);
const actualAdditions = difference(finalKeys, baselineKeys);
const unexpectedRetired = difference(actualRetired, retired);
const retiredStillPresent = difference(
	retired.filter((key) => baselineKeys.includes(key)),
	actualRetired,
);
const ambiguousAdditions = actualAdditions.filter(
	(key) => tokens.filter((token) => key.includes(token)).length > 1,
);
const exactOnlyAdditions = actualAdditions.filter(
	(key) => tokens.filter((token) => key.includes(token)).length === 0,
);
const activeExactAdditions = difference(exactAdditions, retired);
const unregisteredAdditions = difference(exactOnlyAdditions, activeExactAdditions);
const missingExactAdditions = difference(activeExactAdditions, exactOnlyAdditions);
const badTokens = tokens.flatMap((token) => {
	const matches = finalKeys.filter((key) => key.includes(token));
	return matches.length === 1 ? [] : [`${token}: ${matches.length}`];
});
const baselineTodos = baselineRows.filter((row) => row.status === "todo").map((row) => row.key);
const finalTodos = finalRows.filter((row) => row.status === "todo").map((row) => row.key);
const unexpectedTodos = [
	...difference(finalTodos, baselineTodos),
	...difference(baselineTodos, finalTodos),
];
const unexpectedStatuses = finalRows
	.filter((row) => row.status !== "passed" && row.status !== "todo")
	.map((row) => `${row.status}: ${row.key}`);
const statusCounts = count(finalRows.map((row) => row.status));

const problems = [
	...unexpectedRetired.map((key) => `unregistered retire: ${key}`),
	...retiredStillPresent.map((key) => `retired test still present: ${key}`),
	...unregisteredAdditions.map((key) => `unregistered addition: ${key}`),
	...ambiguousAdditions.map((key) => `addition matches multiple tokens: ${key}`),
	...missingExactAdditions.map((key) => `registered exact addition is missing: ${key}`),
	...badTokens.map((value) => `registered token match count ${value}`),
	...unexpectedTodos.map((key) => `todo inventory changed: ${key}`),
	...unexpectedStatuses.map((value) => `unexpected test status ${value}`),
];

for (const [name, actual] of Object.entries({
	baseline: baselineKeys.length,
	retired: actualRetired.length,
	additions: actualAdditions.length,
	final: finalKeys.length,
	passed: statusCounts.get("passed") ?? 0,
	todo: statusCounts.get("todo") ?? 0,
	tokens: tokens.length,
})) {
	if (actual !== EXPECTED[name]) problems.push(`${name}: expected ${EXPECTED[name]}, got ${actual}`);
}

console.log(
	`${baselineKeys.length} - ${actualRetired.length} + ${actualAdditions.length} = ${finalKeys.length}`,
);
console.log(
	`passed=${statusCounts.get("passed") ?? 0} todo=${statusCounts.get("todo") ?? 0} tokens=${tokens.length} unexpected=${problems.length}`,
);

if (problems.length > 0) {
	for (const problem of problems) console.error(problem);
	process.exitCode = 1;
}
