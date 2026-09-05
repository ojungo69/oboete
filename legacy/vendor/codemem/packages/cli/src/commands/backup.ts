import * as p from "@clack/prompts";
import { createMcpRpcClient, type McpRpcOutcome } from "@codemem/mcp";
import { Command } from "commander";
import { helpStyle } from "../help-style.js";
import {
	addDbOption,
	addJsonOption,
	type DbOpts,
	emitJsonError,
	type JsonOpts,
	resolveDataDirOpt,
} from "../shared-options.js";
import { runDaemonOperation } from "./daemon-operation.js";

export const BACKUP_PRIVACY_NOTICE =
	"Backups may contain private and local-only data. Phase 1 provides local backups only; off-device backup and export are not available.";

type BackupOpts = DbOpts & JsonOpts;

function client(opts: DbOpts) {
	return createMcpRpcClient({ dataDir: resolveDataDirOpt(opts) });
}

function reportError(
	outcome: Extract<McpRpcOutcome, { ok: false }> & { operationId?: string },
	json = false,
): void {
	const message = outcome.operationId
		? `${outcome.error.message} Operation ID: ${outcome.operationId}`
		: outcome.error.message;
	if (json) emitJsonError(outcome.error.code, message);
	else {
		p.log.error(message);
		process.exitCode = 1;
	}
}

function printResult(result: Record<string, unknown>, json = false): void {
	if (json) {
		console.log(JSON.stringify({ ...result, privacy: BACKUP_PRIVACY_NOTICE }, null, 2));
		return;
	}
	p.log.warn(BACKUP_PRIVACY_NOTICE);
}

const createCommand = new Command("create")
	.configureHelp(helpStyle)
	.description("Create and verify a local backup")
	.option("--reason <reason>", "backup reason", "manual");
addDbOption(createCommand);
addJsonOption(createCommand);
createCommand.action(async (opts: BackupOpts & { reason: string }) => {
	const outcome = await runDaemonOperation(opts, "POST /v1/backup/create", {
		reason: opts.reason,
	});
	if (!outcome.ok) return reportError(outcome, opts.json);
	printResult(outcome.result, opts.json);
	if (!opts.json) p.log.success(`Backup created: ${String(outcome.result.backupId)}`);
});

const listCommand = new Command("list").configureHelp(helpStyle).description("List local backups");
addDbOption(listCommand);
addJsonOption(listCommand);
listCommand.action(async (opts: BackupOpts) => {
	const outcome = await client(opts).request("GET /v1/backup/list", {});
	if (!outcome.ok) return reportError(outcome, opts.json);
	printResult(outcome.result, opts.json);
	if (opts.json) return;
	const backups = Array.isArray(outcome.result.backups)
		? (outcome.result.backups as Array<Record<string, unknown>>)
		: [];
	if (backups.length === 0) {
		p.log.info("No backups found.");
		return;
	}
	for (const backup of backups) {
		const line = `${String(backup.backupId)} · ${String(backup.createdAt ?? "unknown")} · ${backup.valid === true ? "valid" : "invalid"}`;
		if (backup.valid === true) p.log.success(line);
		else p.log.error(line);
	}
});

const verifyCommand = new Command("verify")
	.configureHelp(helpStyle)
	.description("Verify a local backup")
	.argument("<id>", "backup ID");
addDbOption(verifyCommand);
addJsonOption(verifyCommand);
verifyCommand.action(async (backupId: string, opts: BackupOpts) => {
	const outcome = await client(opts).request("POST /v1/backup/verify", { backupId });
	if (!outcome.ok) return reportError(outcome, opts.json);
	printResult(outcome.result, opts.json);
	if (outcome.result.valid !== true) {
		if (!opts.json) p.log.error(`Backup verification failed: ${backupId}`);
		process.exitCode = 1;
		return;
	}
	if (!opts.json) p.log.success(`Backup verified: ${backupId}`);
});

const restoreCommand = new Command("restore")
	.configureHelp(helpStyle)
	.description("Restore a local backup through the daemon journal")
	.argument("<id>", "backup ID");
addDbOption(restoreCommand);
addJsonOption(restoreCommand);
restoreCommand.action(async (backupId: string, opts: BackupOpts) => {
	const outcome = await runDaemonOperation(opts, "POST /v1/backup/restore", { backupId });
	if (!outcome.ok) return reportError(outcome, opts.json);
	printResult(outcome.result, opts.json);
	if (!opts.json)
		p.log.success(`Backup restored: ${backupId}. Restart the daemon to resume service.`);
});

export const backupCommand = new Command("backup")
	.configureHelp(helpStyle)
	.description("Create, list, verify, or restore local backups")
	.addHelpText("after", `\nPrivacy: ${BACKUP_PRIVACY_NOTICE}\n`)
	.addCommand(createCommand)
	.addCommand(listCommand)
	.addCommand(verifyCommand)
	.addCommand(restoreCommand);
