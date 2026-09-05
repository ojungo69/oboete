import { describe, expect, it, vi } from "vitest";

const { request, runDaemonOperation } = vi.hoisted(() => ({
	request: vi.fn(),
	runDaemonOperation: vi.fn(),
}));
vi.mock("@codemem/mcp", () => ({
	createMcpRpcClient: () => ({ request }),
}));
vi.mock("./daemon-operation.js", () => ({ runDaemonOperation }));

import { BACKUP_PRIVACY_NOTICE, backupCommand } from "./backup.js";

describe("backup command", () => {
	it("P1-T052-04-backup-privacy-copy", async () => {
		const output = backupCommand.configureOutput();
		const originalExitCode = process.exitCode;
		const log = vi.spyOn(console, "log").mockImplementation(() => {});
		const stdout = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		let help = "";
		try {
			backupCommand.configureOutput({ writeOut: (value) => (help += value) });
			backupCommand.outputHelp();
			backupCommand.configureOutput(output);

			runDaemonOperation.mockResolvedValueOnce({
				ok: true,
				operationId: "backup-json",
				result: { backupId: "backup-json" },
			});
			await backupCommand.parseAsync(["create", "--reason", "release", "--json"], {
				from: "user",
			});
			expect(runDaemonOperation).toHaveBeenLastCalledWith(
				expect.objectContaining({ json: true, reason: "release" }),
				"POST /v1/backup/create",
				{ reason: "release" },
			);
			expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toMatchObject({
				backupId: "backup-json",
				privacy: BACKUP_PRIVACY_NOTICE,
			});

			runDaemonOperation.mockResolvedValueOnce({
				ok: true,
				operationId: "backup-human",
				result: { backupId: "backup-human" },
			});
			await backupCommand.parseAsync(["create"], { from: "user" });

			request.mockResolvedValueOnce({ ok: true, result: { backups: [] } });
			await backupCommand.parseAsync(["list", "--json"], { from: "user" });
			expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toMatchObject({ backups: [] });
			request.mockResolvedValueOnce({ ok: true, result: {} });
			await backupCommand.parseAsync(["list"], { from: "user" });
			request.mockResolvedValueOnce({
				ok: true,
				result: {
					backups: [
						{ backupId: "valid", createdAt: "2026-08-15T00:00:00.000Z", valid: true },
						{ backupId: "invalid", valid: false },
					],
				},
			});
			await backupCommand.parseAsync(["list"], { from: "user" });

			request.mockResolvedValueOnce({
				ok: true,
				result: { backupId: "valid", valid: true },
			});
			await backupCommand.parseAsync(["verify", "valid", "--json"], { from: "user" });
			request.mockResolvedValueOnce({
				ok: true,
				result: { backupId: "valid", valid: true },
			});
			await backupCommand.parseAsync(["verify", "valid"], { from: "user" });
			request.mockResolvedValueOnce({
				ok: true,
				result: { backupId: "invalid-json", valid: false },
			});
			process.exitCode = 0;
			await backupCommand.parseAsync(["verify", "invalid-json", "--json"], { from: "user" });
			expect(process.exitCode).toBe(1);
			request.mockResolvedValueOnce({
				ok: true,
				result: { backupId: "invalid-human", valid: false },
			});
			process.exitCode = 0;
			await backupCommand.parseAsync(["verify", "invalid-human"], { from: "user" });
			expect(process.exitCode).toBe(1);

			runDaemonOperation.mockResolvedValueOnce({
				ok: true,
				operationId: "restore-json",
				result: { backupId: "restore-json", state: "completed" },
			});
			await backupCommand.parseAsync(["restore", "restore-json", "--json"], { from: "user" });
			expect(runDaemonOperation).toHaveBeenLastCalledWith(
				expect.objectContaining({ json: true }),
				"POST /v1/backup/restore",
				{ backupId: "restore-json" },
			);
			runDaemonOperation.mockResolvedValueOnce({
				ok: true,
				operationId: "restore-human",
				result: { backupId: "restore-human", state: "completed" },
			});
			await backupCommand.parseAsync(["restore", "restore-human"], { from: "user" });

			const failure = {
				ok: false,
				operationId: "lost-operation",
				terminal: false,
				error: { code: "daemon_unavailable", message: "daemon down", retryable: true },
			};
			const failureCases: Array<[string[], boolean]> = [
				[["create", "--json"], true],
				[["list"], false],
				[["verify", "missing", "--json"], true],
				[["restore", "missing"], false],
			];
			for (const [args, json] of failureCases) {
				if (args[0] === "create" || args[0] === "restore") {
					runDaemonOperation.mockResolvedValueOnce(failure);
				} else {
					request.mockResolvedValueOnce({ ok: false, error: failure.error });
				}
				process.exitCode = 0;
				await backupCommand.parseAsync(args, { from: "user" });
				expect(process.exitCode).toBe(1);
				if (json) {
					expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toEqual({
						error: "daemon_unavailable",
						message:
							args[0] === "create" || args[0] === "restore"
								? "daemon down Operation ID: lost-operation"
								: "daemon down",
					});
				}
			}
			expect(stdout.mock.calls.flat().join("")).toContain(BACKUP_PRIVACY_NOTICE);
			expect(stdout.mock.calls.flat().join("")).toContain("Backup created: backup-human");
			expect(stdout.mock.calls.flat().join("")).toContain("No backups found.");
			expect(stdout.mock.calls.flat().join("")).toContain(
				"valid · 2026-08-15T00:00:00.000Z · valid",
			);
			expect(stdout.mock.calls.flat().join("")).toContain("invalid · unknown · invalid");
			expect(stdout.mock.calls.flat().join("")).toContain("Backup verified: valid");
			expect(stdout.mock.calls.flat().join("")).toContain(
				"Backup verification failed: invalid-human",
			);
			expect(stdout.mock.calls.flat().join("")).toContain("Backup restored: restore-human");
			expect(stdout.mock.calls.flat().join("")).toContain("daemon down");
		} finally {
			backupCommand.configureOutput(output);
			process.exitCode = originalExitCode;
			vi.restoreAllMocks();
		}
		expect(help).toContain(BACKUP_PRIVACY_NOTICE);
		expect(BACKUP_PRIVACY_NOTICE).toMatch(/private.*local-only/i);
		expect(BACKUP_PRIVACY_NOTICE).toMatch(/Phase 1.*off-device.*export/i);
	});
});
