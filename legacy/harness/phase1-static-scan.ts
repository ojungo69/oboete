import { readdirSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import ts from "../vendor/codemem/node_modules/typescript/lib/typescript.js";

type Rule =
	| "actor_open"
	| "connect"
	| "ddl"
	| "deep_import"
	| "disposition"
	| "memory_store"
	| "old_direct_path"
	| "public_bypass"
	| "raw_database"
	| "raw_import"
	| "sidecar";

type Violation = { file: string; line: number; rule: Rule; message: string };

const repoRoot = resolve(import.meta.dirname, "..");
const packageSources = resolve(repoRoot, "vendor/codemem/packages");
const dispositionPath = resolve(repoRoot, "evidence/phase1-disposition.md");
const coreIndexPath = "vendor/codemem/packages/core/src/index.ts";
const rpcContractPath = resolve(
	repoRoot,
	"vendor/codemem/packages/core/src/daemon-rpc-contract.ts",
);
const corePackagePath = resolve(repoRoot, "vendor/codemem/packages/core/package.json");

const expectedHits: Record<"actor_open" | "connect" | "memory_store" | "raw_database" | "raw_import", Set<string>> = {
	connect: new Set([
		"vendor/codemem/packages/core/src/daemon-canonical.ts",
		"vendor/codemem/packages/core/src/daemon-jobs.ts",
	]),
	memory_store: new Set([
		"vendor/codemem/packages/core/src/daemon-canonical.ts",
		"vendor/codemem/packages/core/src/daemon-jobs.ts",
	]),
	actor_open: new Set([
		"vendor/codemem/packages/core/src/db.ts",
		"vendor/codemem/packages/core/src/legacy-cutover.ts",
		"vendor/codemem/packages/core/src/online-backup.ts",
		"vendor/codemem/packages/core/src/storage.ts",
	]),
	raw_database: new Set([
		"vendor/codemem/packages/core/src/daemon-lifecycle.ts",
		"vendor/codemem/packages/core/src/writer-actor.ts",
	]),
	raw_import: new Set([
		"vendor/codemem/packages/core/src/daemon-lifecycle.ts",
		"vendor/codemem/packages/core/src/writer-actor.ts",
	]),
};

const expectedDdlFiles = new Set([
	"vendor/codemem/packages/core/src/daemon-jobs-schema.ts",
	"vendor/codemem/packages/core/src/db.ts",
	"vendor/codemem/packages/core/src/maintenance-jobs.ts",
	"vendor/codemem/packages/core/src/maintenance/init-vacuum.ts",
	"vendor/codemem/packages/core/src/mutation-dispatcher.ts",
	"vendor/codemem/packages/core/src/schema-bootstrap.ts",
]);

const forbiddenPublicValues = new Set([
	"connect",
	"connectReadOnly",
	"MemoryStore",
	"ReadOnlyActor",
	"WriterActor",
	"openMigratedWriter",
	"openTestMemoryStore",
	"exportMemories",
	"importMemories",
	"initDatabase",
	"vacuumDatabase",
	"getMemoryArtifactReport",
	"getMemoryRoleReport",
	"getRawEventRelinkPlan",
	"getRawEventRelinkReport",
	"getRawEventStatus",
	"getReliabilityMetrics",
	"rawEventsGate",
	"retryRawEventFailures",
	"DedupKeyBackfillRunner",
	"RefBackfillRunner",
	"ScopeBackfillRunner",
	"SessionContextBackfillRunner",
	"SummaryDedupBackfillRunner",
	"VectorModelMigrationRunner",
]);

const removedDirectNames = new Set([
	"BetterSqliteCoordinatorStore",
	"buildLocalPack",
	"connectCoordinator",
	"directEnqueue",
	"flushBoundaryRawEvents",
]);

const removedSidecarNames = new Set([
	"_buildSidecarCommand",
	"_invokeSidecar",
	"_callSidecar",
	"_buildCodexSidecarCommand",
	"_invokeCodexSidecar",
	"_callCodexSidecar",
	"bypassPermissions",
	"claude_sidecar",
	"codex_sidecar",
]);

const forbiddenDeepModules = new Set([
	"daemon-canonical",
	"daemon-jobs",
	"daemon-lifecycle",
	"db",
	"store",
	"test-utils",
	"writer-actor",
]);

const ddlPattern = /\b(?:CREATE|ALTER|DROP)\s+(?:TABLE|INDEX|TRIGGER|VIEW|VIRTUAL\s+TABLE)\b/i;

function sourcePath(path: string): string {
	return relative(repoRoot, path).replaceAll("\\", "/");
}

function isTestOnly(path: string): boolean {
	return (
		/\.(?:eval\.)?test\.[cm]?[jt]sx?$/.test(path) ||
		path === "vendor/codemem/packages/core/src/test-utils.ts" ||
		path === "vendor/codemem/packages/core/src/test-schema.generated.ts" ||
		path === "vendor/codemem/packages/core/scripts/generate-test-schema.ts"
	);
}

function productionSources(dir: string): string[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = resolve(dir, entry.name);
		if (entry.isDirectory()) {
			return entry.name === "dist" || entry.name === "node_modules" ? [] : productionSources(path);
		}
		const relativePath = sourcePath(path);
		return /\.[cm]?[jt]sx?$/.test(entry.name) && !isTestOnly(relativePath) ? [path] : [];
	});
}

function moduleStem(specifier: string): string {
	const base = specifier.split("/").at(-1) ?? specifier;
	return base.replace(/\.[cm]?[jt]sx?$/, "");
}

function isCoreSource(path: string): boolean {
	return path.startsWith("vendor/codemem/packages/core/src/");
}

function runtimeImportElements(node: ts.ImportDeclaration): Array<{ local: string; original: string }> {
	const clause = node.importClause;
	if (!clause || clause.isTypeOnly) return [];
	const elements: Array<{ local: string; original: string }> = [];
	if (clause.name) elements.push({ local: clause.name.text, original: "default" });
	if (clause.namedBindings && ts.isNamedImports(clause.namedBindings)) {
		for (const element of clause.namedBindings.elements) {
			if (!element.isTypeOnly) {
				elements.push({
					local: element.name.text,
					original: element.propertyName?.text ?? element.name.text,
				});
			}
		}
	}
	return elements;
}

function textValue(node: ts.Node): string | null {
	if (ts.isStringLiteralLike(node)) return node.text;
	if (!ts.isTemplateExpression(node)) return null;
	return `${node.head.text}${node.templateSpans.map((span: ts.TemplateSpan) => span.literal.text).join("")}`;
}

function location(source: ts.SourceFile, node: ts.Node): number {
	return source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
}

function scanSource(file: string, contents: string): { hits: Map<string, Set<string>>; violations: Violation[] } {
	const source = ts.createSourceFile(file, contents, ts.ScriptTarget.Latest, true);
	const violations: Violation[] = [];
	const hits = new Map<string, Set<string>>();
	const imported = new Map<string, string>();
	const namespaces = new Map<string, string>();
	const seen = new Set<string>();

	const hit = (rule: string) => {
		const paths = hits.get(rule) ?? new Set<string>();
		paths.add(file);
		hits.set(rule, paths);
	};
	const fail = (rule: Rule, node: ts.Node, message: string) => {
		const key = `${rule}:${location(source, node)}:${message}`;
		if (seen.has(key)) return;
		seen.add(key);
		violations.push({ file, line: location(source, node), rule, message });
	};
	const inspectRuntimeModule = (node: ts.Node, specifier: string) => {
		if (specifier === "better-sqlite3") hit("raw_import");
		if (
			!isCoreSource(file) &&
			forbiddenDeepModules.has(moduleStem(specifier)) &&
			(specifier.includes("core/src/") || specifier.startsWith("@codemem/core/"))
		) {
			fail("deep_import", node, `daemon-owned core module imported through ${specifier}`);
		}
	};

	for (const statement of source.statements) {
		if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
			const specifier = statement.moduleSpecifier.text;
			const elements = runtimeImportElements(statement);
			for (const element of elements) imported.set(element.local, element.original);
			const bindings = statement.importClause?.namedBindings;
			if (bindings && ts.isNamespaceImport(bindings) && !statement.importClause?.isTypeOnly) {
				namespaces.set(bindings.name.text, moduleStem(specifier));
			}
			if (!statement.importClause?.isTypeOnly) inspectRuntimeModule(statement, specifier);
		}
		if (
			ts.isExportDeclaration(statement) &&
			statement.moduleSpecifier &&
			ts.isStringLiteral(statement.moduleSpecifier)
		) {
			const elements = statement.exportClause;
			const hasRuntimeExport =
				!statement.isTypeOnly &&
				(!elements ||
					!ts.isNamedExports(elements) ||
					elements.elements.some((element: ts.ExportSpecifier) => !element.isTypeOnly));
			if (hasRuntimeExport) {
				inspectRuntimeModule(statement, statement.moduleSpecifier.text);
			}
		}
	}

	const originalName = (node: ts.Expression): string | null => {
		if (ts.isIdentifier(node)) return imported.get(node.text) ?? node.text;
		if (ts.isPropertyAccessExpression(node) && ts.isIdentifier(node.expression)) {
			return namespaces.has(node.expression.text) ? node.name.text : null;
		}
		return null;
	};

	const visit = (node: ts.Node) => {
		if (ts.isCallExpression(node)) {
			const callee = originalName(node.expression);
			if (callee === "connect" || callee === "connectReadOnly") hit("connect");
			const moduleSpecifier = node.arguments[0] ? textValue(node.arguments[0]) : null;
			if (
				moduleSpecifier !== null &&
				(node.expression.kind === ts.SyntaxKind.ImportKeyword ||
					(ts.isIdentifier(node.expression) && node.expression.text === "require"))
			) {
				inspectRuntimeModule(node, moduleSpecifier);
			}
			if (ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === "open") {
				const owner = originalName(node.expression.expression);
				if (owner === "WriterActor" || owner === "ReadOnlyActor") hit("actor_open");
			}
		}
		if (ts.isNewExpression(node)) {
			const name = originalName(node.expression);
			if (name === "MemoryStore") hit("memory_store");
			if (name === "Database" || name === "BetterSqlite3" || name === "default") {
				hit("raw_database");
			}
		}
		if (ts.isIdentifier(node)) {
			if (removedDirectNames.has(node.text)) {
				fail("old_direct_path", node, `removed direct DB path ${node.text} remains`);
			}
			if (removedSidecarNames.has(node.text)) {
				fail("sidecar", node, `removed uncertified sidecar ${node.text} remains`);
			}
		}
		const value = textValue(node);
		if (value !== null) {
			for (const name of removedSidecarNames) {
				if (value.includes(name)) fail("sidecar", node, `removed uncertified sidecar ${name} remains`);
			}
			for (const legacyPath of [".codemem.sqlite", ".opencode-mem.sqlite"]) {
				if (
					value.includes(legacyPath) &&
					file !== "vendor/codemem/packages/core/src/db.ts" &&
					file !== "vendor/codemem/packages/core/src/legacy-cutover.ts"
				) {
					fail("old_direct_path", node, `legacy direct path ${legacyPath} is outside cutover code`);
				}
			}
			const parent = node.parent;
			const directSqlCall =
				ts.isCallExpression(parent) &&
				parent.arguments.includes(node as ts.Expression) &&
				ts.isPropertyAccessExpression(parent.expression) &&
				(parent.expression.name.text === "exec" || parent.expression.name.text === "prepare");
			if (ddlPattern.test(value) || (directSqlCall && /\b(?:VACUUM|REINDEX)\b/i.test(value))) {
				hit("ddl");
			}
		}
		if (file === coreIndexPath && ts.isExportDeclaration(node) && !node.isTypeOnly) {
			// `export * from "x"` は必ず moduleSpecifier を持つが、型は Expression | undefined。
			// undefined を渡すと ts.isStringLiteral が .kind 参照で throw するため先に潰す
			if (!node.exportClause && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) {
				if (forbiddenDeepModules.has(moduleStem(node.moduleSpecifier.text))) {
					fail("public_bypass", node, `wildcard runtime export from ${node.moduleSpecifier.text}`);
				}
			}
			if (node.exportClause && ts.isNamedExports(node.exportClause)) {
				for (const element of node.exportClause.elements) {
					const name = element.propertyName?.text ?? element.name.text;
					if (!element.isTypeOnly && forbiddenPublicValues.has(name)) {
						fail("public_bypass", element, `${name} must not be a runtime export`);
					}
				}
			}
		}
		ts.forEachChild(node, visit);
	};
	ts.forEachChild(source, visit);
	return { hits, violations };
}

function compareExact(
	rule: keyof typeof expectedHits | "ddl",
	actual: Set<string>,
	expected: Set<string>,
): Violation[] {
	const violations: Violation[] = [];
	for (const file of actual) {
		if (!expected.has(file)) {
			violations.push({ file, line: 1, rule, message: `${rule} is outside the disposition allowlist` });
		}
	}
	for (const file of expected) {
		if (!actual.has(file)) {
			violations.push({
				file: "evidence/phase1-disposition.md",
				line: 1,
				rule: "disposition",
				message: `${rule} allowlist is stale; expected live hit ${file}`,
			});
		}
	}
	return violations;
}

function verifyDispositionTable(): Violation[] {
	const contents = readFileSync(dispositionPath, "utf8");
	const rpcContract = readFileSync(rpcContractPath, "utf8");
	const start = contents.indexOf("## T048 DB handle closure");
	const end = contents.indexOf("## T051 legacy cutover surface", start);
	if (start < 0 || end < 0) {
		return [
			{
				file: "evidence/phase1-disposition.md",
				line: 1,
				rule: "disposition",
				message: "T048 disposition section is missing",
			},
		];
	}
	const block = contents.slice(start, end);
	const lines = block.split("\n");
	const tableHeader = lines.findIndex((line) => line === "| opener | production allowlist | disposition |");
	const rows = lines
		.slice(tableHeader + 2)
		.slice(0, 5)
		.map((line) => line.split("|")[1]?.replaceAll("`", "").trim() ?? "");
	const expectedRows = new Set([
		"connect / connectReadOnly",
		"new MemoryStore",
		"WriterActor.open / ReadOnlyActor.open",
		"new BetterSqlite3",
		"test-only opener",
	]);
	const violations: Violation[] = [];
	if (rows.length !== expectedRows.size || rows.some((row) => !expectedRows.has(row))) {
		violations.push({
			file: "evidence/phase1-disposition.md",
			line: contents.slice(0, start).split("\n").length,
			rule: "disposition",
			message: "T048 disposition rows do not exactly match the five audited opener classes",
		});
	}
	for (const file of new Set(Object.values(expectedHits).flatMap((paths) => [...paths]))) {
		const basename = file.split("/").at(-1) ?? file;
		if (!block.includes(basename)) {
			violations.push({
				file: "evidence/phase1-disposition.md",
				line: contents.slice(0, start).split("\n").length,
				rule: "disposition",
				message: `live allowlist path ${basename} is absent from the T048 disposition table`,
			});
		}
	}
	if (!block.includes("packages/core/src/test-utils.ts") || !block.includes("test file")) {
		violations.push({
			file: "evidence/phase1-disposition.md",
			line: contents.slice(0, start).split("\n").length,
			rule: "disposition",
			message: "test-only exception is not an exact path/suffix disposition",
		});
	}
	const rpcStart = rpcContract.indexOf("export const RPC_METHODS = [");
	const rpcEnd = rpcContract.indexOf("] as const", rpcStart);
	const rpcMethods = [
		...rpcContract.slice(rpcStart, rpcEnd).matchAll(/"((?:GET|POST|DELETE) \/v1\/[^\"]+)"/g),
	].map((match) => match[1]);
	if (rpcStart < 0 || rpcEnd < 0 || rpcMethods.length === 0) {
		violations.push({
			file: sourcePath(rpcContractPath),
			line: 1,
			rule: "disposition",
			message: "RPC_METHODS contract could not be read",
		});
	} else {
		for (const method of rpcMethods) {
			if (!contents.includes(`\`${method}\``)) {
				violations.push({
					file: "evidence/phase1-disposition.md",
					line: 1,
					rule: "disposition",
					message: `RPC method ${method} is absent from the disposition table`,
				});
			}
		}
	}
	if (contents.includes("/v1/operations/backup/")) {
		violations.push({
			file: "evidence/phase1-disposition.md",
			line: 1,
			rule: "disposition",
			message: "obsolete /v1/operations/backup/* contract remains",
		});
	}
	return violations;
}

function verifyCorePackageExports(
	manifest: { exports?: unknown } = JSON.parse(readFileSync(corePackagePath, "utf8")),
): Violation[] {
	if (!manifest.exports || typeof manifest.exports !== "object" || Array.isArray(manifest.exports)) {
		return [];
	}
	const violations: Violation[] = [];
	for (const [key, target] of Object.entries(manifest.exports)) {
		if (key === ".") continue;
		const encodedTarget = JSON.stringify(target) ?? "";
		if (
			forbiddenDeepModules.has(moduleStem(key)) ||
			[...forbiddenDeepModules].some((module) => encodedTarget.includes(`/${module}.`))
		) {
			violations.push({
				file: sourcePath(corePackagePath),
				line: 1,
				rule: "public_bypass",
				message: `package export ${key} exposes a daemon-owned core module`,
			});
		}
	}
	return violations;
}

function runStaticScan(): { files: number; violations: Violation[] } {
	const files = productionSources(packageSources);
	const aggregate = new Map<string, Set<string>>();
	const violations: Violation[] = [];
	for (const path of files) {
		const file = sourcePath(path);
		const result = scanSource(file, readFileSync(path, "utf8"));
		violations.push(...result.violations);
		for (const [rule, paths] of result.hits) {
			const combined = aggregate.get(rule) ?? new Set<string>();
			for (const hit of paths) combined.add(hit);
			aggregate.set(rule, combined);
		}
	}
	for (const [rule, expected] of Object.entries(expectedHits) as Array<[
		keyof typeof expectedHits,
		Set<string>,
	]>) {
		violations.push(...compareExact(rule, aggregate.get(rule) ?? new Set(), expected));
	}
	violations.push(...compareExact("ddl", aggregate.get("ddl") ?? new Set(), expectedDdlFiles));
	violations.push(...verifyDispositionTable());
	violations.push(...verifyCorePackageExports());
	return {
		files: files.length,
		violations: violations.toSorted(
			(left, right) =>
				left.file.localeCompare(right.file) || left.line - right.line || left.rule.localeCompare(right.rule),
		),
	};
}

function selfTest(): void {
	const result = scanSource(
		"vendor/codemem/packages/cli/src/forbidden-fixture.ts",
		`import SqliteAlias from "better-sqlite3";
import { connect as openAlias } from "../../core/src/db.js";
import { MemoryStore as StoreAlias } from "@codemem/core";
const db = new SqliteAlias("/tmp/forbidden.sqlite");
openAlias("/tmp/forbidden.sqlite");
new StoreAlias(db);
db.exec("CREATE TABLE forbidden(value TEXT)");
const runtime = "codex_sidecar";
`,
	);
	const rules = new Set(result.violations.map((item) => item.rule));
	for (const required of ["deep_import", "sidecar"] as const) {
		if (!rules.has(required)) throw new Error(`self-test did not detect ${required}`);
	}
	for (const required of ["connect", "ddl", "memory_store", "raw_database", "raw_import"] as const) {
		if (!result.hits.has(required)) throw new Error(`self-test did not detect ${required}`);
	}
	const dynamic = scanSource(
		"vendor/codemem/packages/cli/src/dynamic-forbidden-fixture.ts",
		`void import("@codemem/core/writer-actor");
const Sqlite = require("better-sqlite3");
`,
	);
	if (!dynamic.violations.some((item) => item.rule === "deep_import")) {
		throw new Error("self-test did not detect dynamic deep_import");
	}
	if (!dynamic.hits.has("raw_import")) {
		throw new Error("self-test did not detect require raw_import");
	}
	if (
		!verifyCorePackageExports({ exports: { ".": "./dist/index.js", "./raw": "./dist/writer-actor.js" } }).some(
			(item) => item.rule === "public_bypass",
		)
	) {
		throw new Error("self-test did not detect package export bypass");
	}
	console.log("PASS phase1-static-scan self-test");
}

if (process.argv.includes("--self-test")) {
	selfTest();
} else {
	const result = runStaticScan();
	if (result.violations.length > 0) {
		for (const violation of result.violations) {
			console.error(`${violation.file}:${violation.line} [${violation.rule}] ${violation.message}`);
		}
		console.error(`FAIL phase1-static-scan: ${result.violations.length} violation(s)`);
		process.exitCode = 1;
	} else {
		console.log(`PASS phase1-static-scan: ${result.files} production source files, 0 violations`);
	}
}
