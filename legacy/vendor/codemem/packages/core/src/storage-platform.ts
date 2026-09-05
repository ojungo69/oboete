import { createHash, randomUUID } from "node:crypto";
import {
	chmodSync,
	closeSync,
	constants,
	copyFileSync,
	existsSync,
	fstatSync,
	fsyncSync,
	linkSync,
	lstatSync,
	mkdirSync,
	openSync,
	readdirSync,
	readFileSync,
	readlinkSync,
	readSync,
	realpathSync,
	renameSync,
	rmdirSync,
	statfsSync,
	symlinkSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";

export type DurableFileImage = { sha256: string; mode: number } | null;
export type DurableRestoreImage = { contents: Uint8Array; mode: number } | null;
export const MAX_CAPABILITY_SETUP_FILE_BYTES = 4_500_000;

export interface DurableRestoreFileInput {
	path: string;
	recoveryId: string;
	expectedAfter: DurableFileImage;
	restoreBefore: DurableRestoreImage;
}

export type InspectedRegularFile =
	| { state: "absent" }
	| { state: "invalid" }
	| {
			state: "regular";
			contents: Buffer;
			sha256: string;
			mode: number;
			dev: number;
			ino: number;
	  };

function classifyRegularFileOpenFailure(path: string, error: unknown): "absent" | "invalid" {
	if ((error as NodeJS.ErrnoException).code !== "ENOENT") return "invalid";
	try {
		lstatSync(path);
	} catch (currentError) {
		return (currentError as NodeJS.ErrnoException).code === "ENOENT" ? "absent" : "invalid";
	}
	return "invalid";
}

export function inspectRegularFile(
	path: string,
	maxBytes = MAX_CAPABILITY_SETUP_FILE_BYTES,
): InspectedRegularFile {
	if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) {
		throw new Error("Invalid regular-file inspection byte limit.");
	}
	let descriptor: number;
	try {
		descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
	} catch (error) {
		return { state: classifyRegularFileOpenFailure(path, error) };
	}
	try {
		const opened = fstatSync(descriptor);
		if (
			!opened.isFile() ||
			!Number.isSafeInteger(opened.size) ||
			opened.size < 0 ||
			opened.size > maxBytes
		) {
			return { state: "invalid" };
		}
		const contents = Buffer.alloc(opened.size);
		let offset = 0;
		while (offset < contents.length) {
			const bytesRead = readSync(descriptor, contents, offset, contents.length - offset, offset);
			if (bytesRead === 0) return { state: "invalid" };
			offset += bytesRead;
		}
		if (readSync(descriptor, Buffer.allocUnsafe(1), 0, 1, contents.length) !== 0) {
			return { state: "invalid" };
		}
		const finalOpened = fstatSync(descriptor);
		if (finalOpened.size !== opened.size) return { state: "invalid" };
		const current = lstatSync(path);
		if (
			current.isSymbolicLink() ||
			current.dev !== finalOpened.dev ||
			current.ino !== finalOpened.ino
		) {
			return { state: "invalid" };
		}
		return {
			state: "regular",
			contents,
			sha256: createHash("sha256").update(contents).digest("hex"),
			mode: finalOpened.mode & 0o777,
			dev: finalOpened.dev,
			ino: finalOpened.ino,
		};
	} catch {
		return { state: "invalid" };
	} finally {
		closeSync(descriptor);
	}
}

function matchesFileImage(inspected: InspectedRegularFile, expected: DurableFileImage): boolean {
	return expected === null
		? inspected.state === "absent"
		: inspected.state === "regular" &&
				inspected.mode === expected.mode &&
				inspected.sha256 === expected.sha256;
}

function removeKnownRecoveryFile(path: string): void {
	try {
		unlinkSync(path);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
	}
}

function isKnownRecoveryFile(state: InspectedRegularFile, expected: DurableFileImage): boolean {
	return state.state === "absent" || (expected !== null && matchesFileImage(state, expected));
}

function durableRestoreContext(input: DurableRestoreFileInput): {
	parent: string;
	recoveryDir: string;
	claim: string;
	prior: string;
	expectedBefore: DurableFileImage;
} {
	if (!/^[a-f0-9]{32}$/.test(input.recoveryId)) throw new Error("Invalid setup recovery ID.");
	const parent = dirname(input.path);
	const recoveryDir = `${parent}/.codemem-setup-restore-${input.recoveryId}`;
	return {
		parent,
		recoveryDir,
		claim: `${recoveryDir}/claim`,
		prior: `${recoveryDir}/prior`,
		expectedBefore:
			input.restoreBefore === null
				? null
				: {
						sha256: createHash("sha256").update(input.restoreBefore.contents).digest("hex"),
						mode: input.restoreBefore.mode,
					},
	};
}

function inspectRecoveryDirectory(path: string): "absent" | "valid" | "invalid" {
	let state: ReturnType<typeof lstatSync>;
	try {
		state = lstatSync(path);
	} catch (error) {
		return (error as NodeJS.ErrnoException).code === "ENOENT" ? "absent" : "invalid";
	}
	if (
		state.isSymbolicLink() ||
		!state.isDirectory() ||
		(state.mode & 0o777) !== 0o700 ||
		(typeof process.getuid === "function" && state.uid !== process.getuid())
	) {
		return "invalid";
	}
	try {
		return readdirSync(path).every((entry) => entry === "claim" || entry === "prior")
			? "valid"
			: "invalid";
	} catch {
		return "invalid";
	}
}

function ensureRecoveryDirectory(path: string, parent: string): boolean {
	const state = inspectRecoveryDirectory(path);
	if (state === "valid") return true;
	if (state === "invalid") return false;
	let created = false;
	try {
		mkdirSync(path, { mode: 0o700 });
		created = true;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
	}
	if (created) {
		chmodSync(path, 0o700);
		fsyncPath(parent);
	}
	return inspectRecoveryDirectory(path) === "valid";
}

export type DurableRestoreSidecarState = "clean" | "recoverable" | "conflict";

export function inspectDurableRestoreSidecars(
	input: DurableRestoreFileInput,
): DurableRestoreSidecarState {
	const { recoveryDir, claim, prior, expectedBefore } = durableRestoreContext(input);
	const directoryState = inspectRecoveryDirectory(recoveryDir);
	if (directoryState === "invalid") return "conflict";
	if (directoryState === "absent") return "clean";
	const claimState = inspectRegularFile(claim);
	const priorState = inspectRegularFile(prior);
	if (
		!isKnownRecoveryFile(claimState, input.expectedAfter) ||
		!isKnownRecoveryFile(priorState, expectedBefore)
	) {
		return "conflict";
	}
	return claimState.state === "absent" && priorState.state === "absent" ? "clean" : "recoverable";
}

export function durableRestoreCanResume(input: DurableRestoreFileInput): boolean {
	if (inspectRegularFile(input.path).state !== "absent") return false;
	if (input.expectedAfter === null) return true;
	const { claim } = durableRestoreContext(input);
	return matchesFileImage(inspectRegularFile(claim), input.expectedAfter);
}

function clearValidatedRecoveryFiles(
	recoveryDir: string,
	claim: string,
	expectedAfter: DurableFileImage,
	prior: string,
	expectedBefore: DurableFileImage,
	parent: string,
): boolean {
	const directoryState = inspectRecoveryDirectory(recoveryDir);
	if (directoryState === "invalid") return false;
	if (directoryState === "absent") return true;
	const files = [
		{ path: claim, expected: expectedAfter, inspected: inspectRegularFile(claim) },
		{ path: prior, expected: expectedBefore, inspected: inspectRegularFile(prior) },
	];
	if (files.some((file) => !isKnownRecoveryFile(file.inspected, file.expected))) return false;
	for (const file of files) {
		if (file.inspected.state === "absent") continue;
		if (file.inspected.state !== "regular") return false;
		const latest = inspectRegularFile(file.path);
		if (
			latest.state !== "regular" ||
			latest.dev !== file.inspected.dev ||
			latest.ino !== file.inspected.ino ||
			!matchesFileImage(latest, file.expected)
		) {
			return false;
		}
		// Node has no conditional unlink; deterministic sidecar names are reserved to this lease.
		removeKnownRecoveryFile(file.path);
	}
	fsyncPath(recoveryDir);
	try {
		rmdirSync(recoveryDir);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "ENOENT") return false;
	}
	fsyncPath(parent);
	return true;
}

export function durableClearRestoreSidecarsIfKnown(input: DurableRestoreFileInput): boolean {
	const { parent, recoveryDir, claim, prior, expectedBefore } = durableRestoreContext(input);
	return clearValidatedRecoveryFiles(
		recoveryDir,
		claim,
		input.expectedAfter,
		prior,
		expectedBefore,
		parent,
	);
}

function removeEmptyRecoveryDirectory(recoveryDir: string, parent: string): void {
	try {
		rmdirSync(recoveryDir);
		fsyncPath(parent);
	} catch (error) {
		if (!["ENOENT", "ENOTEMPTY"].includes((error as NodeJS.ErrnoException).code ?? "")) throw error;
	}
}

function restoreCapturedRecoveryObject(
	claim: string,
	path: string,
	recoveryDir: string,
	parent: string,
): void {
	if (inspectRegularFile(path).state !== "absent") return;
	try {
		linkSync(claim, path);
		fsyncPath(parent);
		removeKnownRecoveryFile(claim);
		fsyncPath(recoveryDir);
		removeEmptyRecoveryDirectory(recoveryDir, parent);
		return;
	} catch (error) {
		const code = (error as NodeJS.ErrnoException).code;
		if (code === "EEXIST" || code === "ENOENT") return;
		if (code !== "EPERM" && code !== "EISDIR") throw error;
	}
	if (inspectRegularFile(path).state !== "absent") return;
	try {
		symlinkSync(claim, path);
		fsyncPath(parent);
	} catch (error) {
		const code = (error as NodeJS.ErrnoException).code;
		if (code === "EEXIST" || code === "ENOENT") return;
		throw error;
	}
}

export function durableRestoreFileIfUnchanged(
	input: DurableRestoreFileInput,
): "restored" | "conflict" {
	const { parent, recoveryDir, claim, prior, expectedBefore } = durableRestoreContext(input);
	if (inspectDurableRestoreSidecars(input) === "conflict") return "conflict";
	let claimState = inspectRegularFile(claim);
	if (!isKnownRecoveryFile(claimState, input.expectedAfter)) return "conflict";

	let current = inspectRegularFile(input.path);
	if (matchesFileImage(current, expectedBefore)) {
		return clearValidatedRecoveryFiles(
			recoveryDir,
			claim,
			input.expectedAfter,
			prior,
			expectedBefore,
			parent,
		)
			? "restored"
			: "conflict";
	}

	if (claimState.state === "regular") {
		current = inspectRegularFile(input.path);
		if (current.state === "absent") {
			// A prior attempt already quarantined the expected post-setup image.
		} else if (matchesFileImage(current, input.expectedAfter)) {
			if (!matchesFileImage(inspectRegularFile(claim), input.expectedAfter)) return "conflict";
			removeKnownRecoveryFile(claim);
			fsyncPath(recoveryDir);
			claimState = { state: "absent" };
		} else {
			return "conflict";
		}
	}
	if (claimState.state === "absent" && input.expectedAfter !== null) {
		if (!matchesFileImage(inspectRegularFile(input.path), input.expectedAfter)) return "conflict";
		if (!ensureRecoveryDirectory(recoveryDir, parent)) return "conflict";
		try {
			// Node lacks RENAME_NOREPLACE; the 0700 recovery directory reserves this name to the lease.
			renameSync(input.path, claim);
			fsyncPath(parent);
			fsyncPath(recoveryDir);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") return "conflict";
			throw error;
		}
		claimState = inspectRegularFile(claim);
		if (!matchesFileImage(claimState, input.expectedAfter)) {
			restoreCapturedRecoveryObject(claim, input.path, recoveryDir, parent);
			return "conflict";
		}
	} else if (
		claimState.state === "absent" &&
		!matchesFileImage(inspectRegularFile(input.path), input.expectedAfter)
	) {
		return "conflict";
	}

	current = inspectRegularFile(input.path);
	if (matchesFileImage(current, expectedBefore)) {
		return clearValidatedRecoveryFiles(
			recoveryDir,
			claim,
			input.expectedAfter,
			prior,
			expectedBefore,
			parent,
		)
			? "restored"
			: "conflict";
	}
	if (current.state !== "absent") return "conflict";
	if (input.restoreBefore === null) {
		return clearValidatedRecoveryFiles(recoveryDir, claim, input.expectedAfter, prior, null, parent)
			? "restored"
			: "conflict";
	}

	let priorState = inspectRegularFile(prior);
	if (priorState.state === "absent") {
		if (!ensureRecoveryDirectory(recoveryDir, parent)) return "conflict";
		let createdPrior = false;
		try {
			writeFileSync(prior, input.restoreBefore.contents, {
				flag: "wx",
				mode: input.restoreBefore.mode,
				flush: true,
			});
			createdPrior = true;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
		}
		if (createdPrior) {
			chmodSync(prior, input.restoreBefore.mode);
			fsyncPath(recoveryDir);
		}
	}
	priorState = inspectRegularFile(prior);
	if (!matchesFileImage(priorState, expectedBefore)) return "conflict";
	try {
		linkSync(prior, input.path);
		fsyncPath(parent);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
		return "conflict";
	}
	if (!matchesFileImage(inspectRegularFile(input.path), expectedBefore)) return "conflict";
	return clearValidatedRecoveryFiles(
		recoveryDir,
		claim,
		input.expectedAfter,
		prior,
		expectedBefore,
		parent,
	)
		? "restored"
		: "conflict";
}

export function assertSupportedStoragePlatform(): void {
	if (process.platform !== "linux") {
		throw new Error(`Local storage is supported only on Linux/WSL; got ${process.platform}.`);
	}
}

function assertNotSymlinkDirectory(path: string): void {
	const info = lstatSync(path);
	if (info.isSymbolicLink()) {
		throw new Error(`data_dir preflight rejected a symbolic link: ${path}`);
	}
	if (!info.isDirectory()) {
		throw new Error(`Private path is not a directory: ${path}`);
	}
}

export function ensurePrivateDirectory(path: string): void {
	assertSupportedStoragePlatform();
	let existing: ReturnType<typeof lstatSync> | undefined;
	try {
		existing = lstatSync(path);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
	}
	if (existing) {
		assertNotSymlinkDirectory(path);
	} else {
		mkdirSync(path, { recursive: true, mode: 0o700 });
		assertNotSymlinkDirectory(path);
	}
	chmodSync(path, 0o700);
	if (isNetworkFilesystemType(statfsSync(path).type)) {
		throw new Error("data_dir preflight rejected a network filesystem.");
	}
	const fstype = mountFstypeFor(path);
	if (fstype && isForbiddenMountFstype(fstype)) {
		throw new Error("data_dir preflight rejected a network filesystem.");
	}
}

export function fsyncPath(path: string): void {
	const fd = openSync(path, "r");
	try {
		fsyncSync(fd);
	} finally {
		closeSync(fd);
	}
}

export function durableReplaceFile(path: string, contents: string): void {
	const parent = dirname(path);
	ensurePrivateDirectory(parent);
	const temporaryPath = `${path}.${process.pid}.${randomUUID()}.tmp`;
	try {
		writeFileSync(temporaryPath, contents, {
			encoding: "utf8",
			flag: "wx",
			mode: 0o600,
			flush: true,
		});
		renameSync(temporaryPath, path);
		fsyncPath(parent);
	} catch (error) {
		try {
			unlinkSync(temporaryPath);
		} catch {
			// The temporary file may not have been created or may already be renamed.
		}
		throw error;
	}
}

export function durableRemoveFile(path: string): void {
	try {
		lstatSync(path);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
		throw error;
	}
	unlinkSync(path);
	fsyncPath(dirname(path));
}

export function durableReplaceSymlink(path: string, target: string): void {
	const parent = dirname(path);
	ensurePrivateDirectory(parent);
	const temporaryPath = `${path}.${process.pid}.${randomUUID()}.tmp`;
	try {
		symlinkSync(target, temporaryPath);
		renameSync(temporaryPath, path);
		fsyncPath(parent);
	} catch (error) {
		try {
			unlinkSync(temporaryPath);
		} catch {
			// The temporary link may not have been created or may already be renamed.
		}
		throw error;
	}
}

export function durableCopyFile(source: string, destination: string): void {
	ensurePrivateDirectory(dirname(destination));
	copyFileSync(source, destination, constants.COPYFILE_EXCL);
	chmodSync(destination, 0o600);
	fsyncPath(destination);
	fsyncPath(dirname(destination));
}

const NETWORK_FS_TYPES = new Set<number>([
	0x6969, // NFS
	0x517b, // SMB
	0xff534d42, // CIFS
	0xfe534d42, // SMB2
	0x01021997, // 9P
	0x65735546, // FUSE (sshfs / rclone / gvfs / s3fs)
	0x6a656a63, // virtiofs
	0x00c36400, // Ceph
	0x6b414653, // kAFS
	0x5346414f, // AFS
	0x0bd00bd0, // Lustre
	0x01161970, // GFS2
	0x7461636f, // OCFS2
]);

const FORBIDDEN_MOUNT_FSTYPES = new Set([
	"nfs",
	"nfs4",
	"cifs",
	"smb3",
	"smbfs",
	"9p",
	"drvfs",
	"virtiofs",
	"ceph",
	"afs",
	"lustre",
	"gfs2",
	"ocfs2",
]);

export function isNetworkFilesystemType(type: number | bigint): boolean {
	const numeric = typeof type === "bigint" ? Number(type) : type;
	return NETWORK_FS_TYPES.has(numeric >>> 0);
}

export function isForbiddenMountFstype(fstype: string): boolean {
	const normalized = fstype.toLowerCase();
	return normalized.startsWith("fuse") || FORBIDDEN_MOUNT_FSTYPES.has(normalized);
}

export function isWslWindowsSharePath(path: string): boolean {
	const normalized = resolve(path).replaceAll("\\", "/").toLowerCase();
	return /^\/mnt\/[a-z](\/|$)/.test(normalized);
}

function existingAncestor(path: string): string {
	let current = resolve(path);
	for (;;) {
		if (existsSync(current)) return current;
		const parent = dirname(current);
		if (parent === current) return current;
		current = parent;
	}
}

function decodeMountinfoPath(value: string): string {
	return value.replace(/\\([0-7]{3})/g, (_match, digits: string) =>
		String.fromCharCode(Number.parseInt(digits, 8)),
	);
}

function mountFstypeFor(path: string): string | null {
	let mountInfo: string;
	try {
		mountInfo = readFileSync("/proc/self/mountinfo", "utf8");
	} catch {
		return null;
	}
	const resolved = resolve(path);
	let best: { mountPoint: string; fstype: string } | null = null;
	for (const line of mountInfo.split("\n")) {
		if (!line) continue;
		const separator = line.indexOf(" - ");
		if (separator < 0) continue;
		const left = line.slice(0, separator).split(" ");
		const right = line.slice(separator + 3).split(" ");
		const mountPoint = decodeMountinfoPath(left[4] ?? "");
		const fstype = right[0] ?? "";
		if (!mountPoint || !fstype) continue;
		const prefix = mountPoint.endsWith("/") ? mountPoint : `${mountPoint}/`;
		if (resolved !== mountPoint && !resolved.startsWith(prefix) && mountPoint !== "/") {
			continue;
		}
		if (mountPoint === "/" && resolved !== "/" && !resolved.startsWith("/")) continue;
		if (!best || mountPoint.length > best.mountPoint.length) {
			best = { mountPoint, fstype };
		}
	}
	return best?.fstype ?? null;
}

export function assertDataDirPreflight(dataDir: string): void {
	assertSupportedStoragePlatform();
	if (isWslWindowsSharePath(dataDir)) {
		throw new Error("data_dir preflight rejected a WSL-Windows share path.");
	}
	const probe = existingAncestor(dataDir);
	if (isNetworkFilesystemType(statfsSync(probe).type)) {
		throw new Error("data_dir preflight rejected a network filesystem.");
	}
	const fstype = mountFstypeFor(probe);
	if (fstype && isForbiddenMountFstype(fstype)) {
		throw new Error("data_dir preflight rejected a network filesystem.");
	}
}

export function readProcessIdentity(pid: number): { startTime: string; fingerprint: string } {
	const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
	const closeParen = stat.lastIndexOf(")");
	if (closeParen < 0) throw new Error(`Cannot parse /proc/${pid}/stat.`);
	const startTime = stat.slice(closeParen + 2).split(" ")[19];
	if (!startTime) throw new Error(`Cannot read start time for pid ${pid}.`);
	let bootId = "";
	try {
		bootId = readFileSync("/proc/sys/kernel/random/boot_id", "utf8").trim();
	} catch {
		// boot_id is a uniqueness aid; starttime still distinguishes PID reuse on one boot.
	}
	const link = readlinkSync(`/proc/${pid}/exe`);
	const target = link.endsWith(" (deleted)") ? link.slice(0, -" (deleted)".length) : link;
	let exe: string;
	try {
		exe = realpathSync(target);
	} catch {
		exe = target;
	}
	const cmdline = readFileSync(`/proc/${pid}/cmdline`);
	return {
		startTime: `${bootId}:${startTime}`,
		fingerprint: createHash("sha256").update(exe).update("\0").update(cmdline).digest("hex"),
	};
}
