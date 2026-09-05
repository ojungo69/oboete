import { randomUUID } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { hashMutationPayload, NORMALIZED_SCHEMA_VERSION, startDaemon } from "@codemem/core";
import { createMcpRpcClient } from "@codemem/mcp";
import { afterEach, describe, expect, it, vi } from "vitest";
import { backupCommand } from "./backup.js";
import { resolveOperationFilePath, runDaemonOperation } from "./daemon-operation.js";
import { dbCommand } from "./db.js";
import { distillCommand } from "./distill.js";
import { embedCommand } from "./embed.js";
import { exportMemoriesCommand } from "./export-memories.js";
import { importMemoriesCommand } from "./import-memories.js";
import { maintenanceCommand } from "./maintenance.js";
import {
	forgetMemoryCommand,
	memoryCommand,
	rememberMemoryCommand,
	showMemoryCommand,
} from "./memory.js";
import { packCommand } from "./pack.js";
import { recentCommand } from "./recent.js";
import { searchCommand } from "./search.js";
import { statsCommand } from "./stats.js";
import { createStatusCommand } from "./status.js";

const cleanup: string[] = [];
const originalDataDir = process.env.CODEMEM_DATA_DIR;
const originalProject = process.env.CODEMEM_PROJECT;
const originalTrace = process.env.CODEMEM_DB_OPEN_TRACE;

afterEach(() => {
	if (originalDataDir === undefined) delete process.env.CODEMEM_DATA_DIR;
	else process.env.CODEMEM_DATA_DIR = originalDataDir;
	if (originalProject === undefined) delete process.env.CODEMEM_PROJECT;
	else process.env.CODEMEM_PROJECT = originalProject;
	if (originalTrace === undefined) delete process.env.CODEMEM_DB_OPEN_TRACE;
	else process.env.CODEMEM_DB_OPEN_TRACE = originalTrace;
	for (const path of cleanup.splice(0)) rmSync(path, { recursive: true, force: true });
	process.exitCode = 0;
	vi.restoreAllMocks();
});

function fixture(prefix: string): { root: string; dataDir: string } {
	const root = mkdtempSync(join(tmpdir(), prefix));
	cleanup.push(root);
	mkdirSync(join(root, ".git"));
	return { root, dataDir: join(root, "data") };
}

describe("Phase 1 CLI RPC cutover", () => {
	it("T031 exposes project filters but no caller-controlled trust authority", () => {
		const authorityFlags = new Set([
			"--execution-location",
			"--local-trust",
			"--model-local",
			"--provider-peer-trust",
			"--repository-identity",
		]);
		for (const command of [
			searchCommand,
			recentCommand,
			packCommand,
			...packCommand.commands,
			showMemoryCommand,
			memoryCommand.commands.find((candidate) => candidate.name() === "inject"),
		]) {
			if (!command) throw new Error("expected CLI read command");
			const flags = command.options.map((option) => option.long);
			expect(
				flags.some((flag) => typeof flag === "string" && authorityFlags.has(flag)),
				command.name(),
			).toBe(false);
		}
		expect(searchCommand.options.map((option) => option.long)).toContain("--project");
		expect(recentCommand.options.map((option) => option.long)).toContain("--project");
		expect(packCommand.options.map((option) => option.long)).toContain("--project");
	});

	it("T032 keeps export destination authority internal", () => {
		const flags = exportMemoriesCommand.options.map((option) => option.long);
		expect(flags).toEqual(expect.arrayContaining(["--project", "--all-projects"]));
		for (const forbidden of [
			"--execution-location",
			"--local-trust",
			"--provider-peer-trust",
			"--repository-identity",
		]) {
			expect(flags).not.toContain(forbidden);
		}
		expect(importMemoriesCommand.options.map((option) => option.long)).not.toContain(
			"--repository-identity",
		);
	});

	it("P1-T044-01-cli-rpc-map", async () => {
		const { root, dataDir } = fixture("codemem-cli-rpc-map-");
		process.env.CODEMEM_DATA_DIR = dataDir;
		delete process.env.CODEMEM_PROJECT;
		const output: string[] = [];
		const humanOutput: string[] = [];
		vi.spyOn(console, "log").mockImplementation((value) => output.push(String(value)));
		vi.spyOn(console, "error").mockImplementation(() => {});
		vi.spyOn(process.stdout, "write").mockImplementation((value) => {
			humanOutput.push(String(value));
			return true;
		});
		const daemon = await startDaemon({ dataDir });
		try {
			const client = createMcpRpcClient({ dataDir, cwd: () => root });
			const exportPath = join(root, "memories.json");
			const filteredExportPath = join(root, "memories-filtered.json");
			const importPath = join(root, "memories-import.json");
			const invalidImportPath = join(root, "invalid-import.json");
			await rememberMemoryCommand.parseAsync(
				[
					"--kind",
					"decision",
					"--title",
					"Use daemon RPC",
					"--body",
					"CLI commands do not open SQLite.",
					"--project",
					"demo",
					"--json",
				],
				{ from: "user" },
			);
			const id = Number((JSON.parse(output.at(-1) ?? "{}") as { id?: number }).id);
			expect(id).toBeGreaterThan(0);

			await showMemoryCommand.parseAsync([String(id), "--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({ id });
			await packCommand.parseAsync(["daemon rpc", "--json", "--all-projects"], {
				from: "user",
			});
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({ pack_text: expect.any(String) });
			await searchCommand.parseAsync(["daemon rpc", "--json", "--all-projects"], {
				from: "user",
			});
			expect(JSON.parse(output.at(-1) ?? "[]")).toEqual(expect.any(Array));
			await recentCommand.parseAsync(["--json", "--all-projects"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "[]")).toEqual(expect.any(Array));
			await statsCommand.parseAsync(["--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({ database: expect.any(Object) });
			await createStatusCommand().parseAsync(["--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({
				daemon: { state: "running" },
				database: { state: "ready" },
			});
			await backupCommand.parseAsync(["create", "--reason", "cli-rpc-test", "--json"], {
				from: "user",
			});
			const backupId = String(
				(JSON.parse(output.at(-1) ?? "{}") as { backupId?: unknown }).backupId,
			);
			expect(backupId).toMatch(/^[A-Za-z0-9._-]+$/);
			await backupCommand.parseAsync(["list", "--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({
				backups: [expect.objectContaining({ backupId, valid: true })],
			});
			await backupCommand.parseAsync(["verify", backupId, "--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({ backupId, valid: true });

			await exportMemoriesCommand.parseAsync([exportPath, "--all-projects"], { from: "user" });
			const exported = JSON.parse(readFileSync(exportPath, "utf8")) as Record<string, unknown>;
			expect(exported).toMatchObject({
				version: "2.0",
				memory_items: [expect.objectContaining({ id, title: "Use daemon RPC" })],
			});
			const exportedSession = (exported.sessions as Array<Record<string, unknown>>)[0];
			for (const forbidden of ["cwd", "git_remote", "git_branch", "user", "metadata_json"]) {
				expect(exportedSession).not.toHaveProperty(forbidden);
			}
			await exportMemoriesCommand.parseAsync(
				[
					filteredExportPath,
					"--project",
					"demo",
					"--include-inactive",
					"--since",
					"2000-01-01T00:00:00.000Z",
				],
				{ from: "user" },
			);
			expect(JSON.parse(readFileSync(filteredExportPath, "utf8"))).toMatchObject({
				export_metadata: {
					include_inactive: true,
					filters: { project: "demo", since: "2000-01-01T00:00:00.000Z" },
				},
				memory_items: [expect.objectContaining({ id })],
			});
			await importMemoriesCommand.parseAsync([exportPath, "--dry-run", "--json"], {
				from: "user",
			});
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({
				memory_items: 1,
				skipped: true,
			});
			writeFileSync(invalidImportPath, "{\n", { mode: 0o600 });
			await importMemoriesCommand.parseAsync([invalidImportPath, "--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({
				error: "invalid_import",
				message: expect.stringContaining("Operation ID:"),
			});
			expect(process.exitCode).toBe(1);
			process.exitCode = 0;
			await maintenanceCommand.parseAsync(["status", "--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({ jobs: expect.any(Array) });
			await maintenanceCommand.parseAsync(["status"], { from: "user" });
			expect(humanOutput.join("")).toContain("codemem maintenance");
			expect(humanOutput.join("")).toContain("completed");
			const importPayload = JSON.parse(readFileSync(exportPath, "utf8")) as {
				memory_items: Array<Record<string, unknown>>;
			};
			for (const memory of importPayload.memory_items) {
				memory.import_key = `cli-rpc-import-${String(memory.id)}`;
			}
			writeFileSync(importPath, JSON.stringify(importPayload), { mode: 0o600 });
			await importMemoriesCommand.parseAsync([importPath, "--remap-project", "imported-demo"], {
				from: "user",
			});
			expect(humanOutput.join("")).toContain("Imported memories:  1");
			await statsCommand.parseAsync(["--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toMatchObject({
				database: { memory_items: 2 },
			});

			const event = {
				schemaVersion: NORMALIZED_SCHEMA_VERSION,
				eventId: randomUUID(),
				idempotencyKey: randomUUID(),
				agent: "opencode",
				nativeSessionId: "session-t044",
				projectKey: "demo",
				workspaceKey: root,
				cwd: root,
				kind: "user_prompted",
				occurredAt: new Date().toISOString(),
				payload: { text: "safe prompt" },
				sourceHash: hashMutationPayload({ text: "safe prompt" }),
				sensitivity: "normal",
			};
			expect(
				await client.requestWithSpool("POST /v1/events", {
					idempotencyKey: event.idempotencyKey,
					event,
				}),
			).toMatchObject({ ok: true, result: { receiptId: expect.any(String) } });
			await forgetMemoryCommand.parseAsync([String(id), "--json"], { from: "user" });
			expect(JSON.parse(output.at(-1) ?? "{}")).toEqual({ id, status: "forgotten" });
		} finally {
			await daemon.stop();
		}
	});

	it("P1-T044-02-cli-typed-stubs", async () => {
		const output: unknown[] = [];
		vi.spyOn(console, "log").mockImplementation((value) => output.push(value));
		vi.spyOn(console, "error").mockImplementation(() => {});
		const extractionReplay = memoryCommand.commands.find(
			(command) => command.name() === "extraction-replay",
		);
		const extractionBenchmark = memoryCommand.commands.find(
			(command) => command.name() === "extraction-benchmark",
		);
		if (!extractionReplay || !extractionBenchmark) throw new Error("typed stub command missing");

		await distillCommand.parseAsync(["--json"], { from: "user" });
		await embedCommand.parseAsync(["--json"], { from: "user" });
		await extractionReplay.parseAsync(["--batch-id", "1", "--scenario", "x", "--json"], {
			from: "user",
		});
		await extractionBenchmark.parseAsync(["--benchmark", "x", "--json"], { from: "user" });

		expect(output.map((value) => JSON.parse(String(value)))).toEqual([
			expect.objectContaining({ code: "feature_unavailable", phase: 6 }),
			expect.objectContaining({ code: "feature_unavailable", phase: 7 }),
			expect.objectContaining({ code: "feature_unavailable", phase: 6 }),
			expect.objectContaining({ code: "feature_unavailable", phase: 6 }),
		]);
	});

	it("P1-T044-03-cli-no-db-fallback", async () => {
		const { root, dataDir } = fixture("codemem-cli-no-db-");
		const tracePath = join(root, "db-open.jsonl");
		process.env.CODEMEM_DATA_DIR = dataDir;
		process.env.CODEMEM_DB_OPEN_TRACE = tracePath;
		vi.spyOn(console, "log").mockImplementation(() => {});
		vi.spyOn(console, "error").mockImplementation(() => {});
		const inject = memoryCommand.commands.find((command) => command.name() === "inject");
		if (!inject) throw new Error("inject command missing");
		const dbSubcommand = (name: string) => {
			const command = dbCommand.commands.find((candidate) => candidate.name() === name);
			if (!command) throw new Error(`db ${name} command missing`);
			return command;
		};
		const memorySubcommand = (name: string) => {
			const command = memoryCommand.commands.find((candidate) => candidate.name() === name);
			if (!command) throw new Error(`memory ${name} command missing`);
			return command;
		};
		const legacyDb = join(root, "legacy.sqlite");

		const commands: Array<() => Promise<unknown>> = [
			() => searchCommand.parseAsync(["query", "--json"], { from: "user" }),
			() => recentCommand.parseAsync(["--json"], { from: "user" }),
			() => statsCommand.parseAsync(["--json"], { from: "user" }),
			() => packCommand.parseAsync(["query", "--json"], { from: "user" }),
			() => showMemoryCommand.parseAsync(["1", "--json"], { from: "user" }),
			() => forgetMemoryCommand.parseAsync(["1", "--json"], { from: "user" }),
			() => inject.parseAsync(["query"], { from: "user" }),
			() => createStatusCommand().parseAsync(["--json"], { from: "user" }),
			() =>
				maintenanceCommand.parseAsync(["status", "--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() => dbSubcommand("init").parseAsync(["--db-path", legacyDb], { from: "user" }),
			() => dbSubcommand("vacuum").parseAsync(["--db-path", legacyDb], { from: "user" }),
			() =>
				dbSubcommand("prune-raw-events").parseAsync(["--dry-run", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("raw-events-status").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("raw-events-retry").parseAsync(["--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("raw-events-gate").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("size-report").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("rename-project").parseAsync(["old", "new", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("normalize-projects").parseAsync(["--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("backfill-tags").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("backfill-dedup-keys").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("backfill-narrative").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("ai-backfill-structured").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("prune-observations").parseAsync(
					["--dry-run", "--json", "--db-path", legacyDb],
					{ from: "user" },
				),
			() =>
				dbSubcommand("prune-memories").parseAsync(["--dry-run", "--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("dedup-memories").parseAsync(["--dry-run", "--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				dbSubcommand("scan-secrets").parseAsync(["--dry-run", "--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				memorySubcommand("role-report").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				memorySubcommand("role-compare").parseAsync([legacyDb, legacyDb, "--json"], {
					from: "user",
				}),
			() =>
				memorySubcommand("artifact-report").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				memorySubcommand("extraction-report").parseAsync(
					[
						"--session-id",
						"1",
						"--scenario",
						"simple-batch-shape",
						"--json",
						"--db-path",
						legacyDb,
					],
					{ from: "user" },
				),
			() =>
				memorySubcommand("relink-report").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				memorySubcommand("relink-plan").parseAsync(["--json", "--db-path", legacyDb], {
					from: "user",
				}),
			() =>
				rememberMemoryCommand.parseAsync(
					["--kind", "decision", "--title", "queued", "--body", "safe", "--json"],
					{ from: "user" },
				),
		];
		for (const run of commands) {
			process.exitCode = 0;
			await run();
		}
		expect(resolveOperationFilePath("~/x")).toBe(join(homedir(), "x"));
		const absentOperation = await runDaemonOperation({}, "POST /v1/operations/export", {
			outputPath: join(root, "never-exported.json"),
			filters: {},
		});
		expect(absentOperation).toMatchObject({
			ok: false,
			operationId: expect.stringMatching(/^[0-9a-f-]{36}$/),
			terminal: false,
			error: { code: "daemon_unavailable", retryable: true },
		});
		expect(existsSync(join(root, "never-exported.json"))).toBe(false);

		const paths = readdirSync(root, { recursive: true }).map(String);
		expect(paths).not.toContain("db-open.jsonl");
		expect(paths.filter((path) => /\.sqlite(?:3)?$/.test(path))).toEqual([]);
		expect(paths.some((path) => path.includes("control/spool/ready"))).toBe(true);
	});
});
