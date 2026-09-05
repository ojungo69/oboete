import { randomUUID } from "node:crypto";
import {
	chmodSync,
	closeSync,
	constants,
	existsSync,
	fstatSync,
	fsyncSync,
	linkSync,
	lstatSync,
	mkdirSync,
	openSync,
	readFileSync,
	renameSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { stripJsonComments, stripTrailingCommas } from "@codemem/core";

export type SetupFileMutation = {
	contents: Uint8Array | null;
	mode: number | null;
};

export type SetupFileSnapshot = SetupFileMutation & {
	path: string;
	mode: number;
	dev: number | null;
	ino: number | null;
};

let activeSetupFileTracking: {
	mutations: Map<string, SetupFileMutation>;
	baselines: ReadonlyMap<string, SetupFileSnapshot>;
} | null = null;

export function withSetupFileMutationTracking<T>(
	mutations: Map<string, SetupFileMutation>,
	baselines: ReadonlyMap<string, SetupFileSnapshot>,
	action: () => T,
): T {
	const previous = activeSetupFileTracking;
	activeSetupFileTracking = { mutations, baselines };
	try {
		return action();
	} finally {
		activeSetupFileTracking = previous;
	}
}

export function recordSetupFileMutation(
	path: string,
	contents: string | Uint8Array | null,
	mode: number | null,
): void {
	if (!activeSetupFileTracking) return;
	const resolvedPath = resolve(path);
	activeSetupFileTracking.mutations.set(resolvedPath, {
		contents: contents === null ? null : Buffer.from(contents),
		mode,
	});
}

export function captureSetupFileSnapshots(paths: readonly string[]): SetupFileSnapshot[] {
	const unique = [...new Set(paths.map((path) => resolve(path)))];
	return unique.map((path) => {
		let descriptor: number;
		try {
			descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") {
				try {
					lstatSync(path);
				} catch (currentError) {
					if ((currentError as NodeJS.ErrnoException).code === "ENOENT") {
						return { path, contents: null, mode: 0o600, dev: null, ino: null };
					}
					throw currentError;
				}
				throw new Error(`Setup transaction target changed while being snapshotted: ${path}`);
			}
			throw error;
		}
		try {
			const opened = fstatSync(descriptor);
			if (!opened.isFile()) {
				throw new Error(`Setup transaction target is not a regular file: ${path}`);
			}
			const contents = readFileSync(descriptor);
			const current = lstatSync(path);
			if (current.isSymbolicLink() || current.dev !== opened.dev || current.ino !== opened.ino) {
				throw new Error(`Setup transaction target changed while being snapshotted: ${path}`);
			}
			return {
				path,
				contents,
				mode: opened.mode & 0o777,
				dev: opened.dev,
				ino: opened.ino,
			};
		} finally {
			closeSync(descriptor);
		}
	});
}

export function setupFileSnapshotUnchanged(snapshot: SetupFileSnapshot): boolean {
	const current = captureSetupFileSnapshots([snapshot.path])[0];
	if (current?.dev !== snapshot.dev || current?.ino !== snapshot.ino) return false;
	if (snapshot.contents === null || current.contents === null) {
		return snapshot.contents === null && current.contents === null;
	}
	return (
		(process.platform === "win32" || current.mode === snapshot.mode) &&
		Buffer.from(current.contents).equals(Buffer.from(snapshot.contents))
	);
}

export function setupFileMatchesMutation(path: string, mutation: SetupFileMutation): boolean {
	try {
		const current = captureSetupFileSnapshots([path])[0];
		if (!current) return false;
		if (mutation.contents === null || current.contents === null) {
			return mutation.contents === null && current.contents === null;
		}
		return (
			(process.platform === "win32" || current.mode === mutation.mode) &&
			Buffer.from(current.contents).equals(Buffer.from(mutation.contents))
		);
	} catch {
		return false;
	}
}

function assertAllTrackedSetupFilesUnchanged(resolvedPath: string): void {
	if (!activeSetupFileTracking) return;
	for (const [trackedPath, baseline] of activeSetupFileTracking.baselines) {
		const mutation = activeSetupFileTracking.mutations.get(trackedPath);
		const unchanged = mutation
			? setupFileMatchesMutation(trackedPath, mutation)
			: setupFileSnapshotUnchanged(baseline);
		if (!unchanged) {
			if (trackedPath === resolvedPath && !mutation) {
				throw new Error(`Setup target changed before its first write: ${trackedPath}`);
			}
			throw new Error(`Setup transaction target changed before mutation: ${trackedPath}`);
		}
	}
}

function assertTrackedSetupFileUnchanged(path: string): SetupFileSnapshot | null {
	const resolvedPath = resolve(path);
	assertAllTrackedSetupFilesUnchanged(resolvedPath);
	const previous = activeSetupFileTracking?.mutations.get(resolvedPath);
	if (previous && !setupFileMatchesMutation(resolvedPath, previous)) {
		throw new Error(`Setup target changed before a repeated write: ${resolvedPath}`);
	}
	if (previous) return null;
	const baseline = activeSetupFileTracking?.baselines.get(resolvedPath);
	if (baseline && !setupFileSnapshotUnchanged(baseline)) {
		throw new Error(`Setup target changed before its first write: ${resolvedPath}`);
	}
	return baseline ?? null;
}

export function assertSetupFileMutationAllowed(path: string): void {
	assertTrackedSetupFileUnchanged(path);
}

export function resolveOpencodeConfigPath(configDir: string): string {
	const jsonPath = join(configDir, "opencode.json");
	if (existsSync(jsonPath)) return jsonPath;
	const jsoncPath = join(configDir, "opencode.jsonc");
	if (existsSync(jsoncPath)) return jsoncPath;
	return jsoncPath;
}

export function parseObjectJson(contents: string): Record<string, unknown> {
	const value: unknown = JSON.parse(contents);
	if (value === null || typeof value !== "object" || Array.isArray(value)) {
		throw new Error("JSON root must be an object");
	}
	return value as Record<string, unknown>;
}

export function loadJsoncConfig(path: string): Record<string, unknown> {
	if (!existsSync(path)) return {};
	const raw = readFileSync(path, "utf-8");
	try {
		return parseObjectJson(raw);
	} catch {
		const cleaned = stripTrailingCommas(stripJsonComments(raw));
		return parseObjectJson(cleaned);
	}
}

export function writeJsonConfig(path: string, data: Record<string, unknown>): void {
	atomicReplaceSetupFile(path, `${JSON.stringify(data, null, 2)}\n`);
}

function fsyncParentBestEffort(path: string): void {
	if (process.platform === "win32") return;
	let descriptor: number | undefined;
	try {
		descriptor = openSync(dirname(path), "r");
		fsyncSync(descriptor);
	} catch {
		// Some supported filesystems do not permit opening/fsyncing directories.
	} finally {
		if (descriptor !== undefined) {
			try {
				closeSync(descriptor);
			} catch {
				// Directory fsync is best-effort on cross-platform editor config paths.
			}
		}
	}
}

export function atomicReplaceSetupFile(
	path: string,
	contents: string | Uint8Array,
	mode?: number,
	exclusive = false,
	expected?: SetupFileSnapshot,
): void {
	if (expected && !setupFileSnapshotUnchanged(expected)) {
		throw new Error(`Setup target changed before replacement: ${resolve(path)}`);
	}
	const baseline = assertTrackedSetupFileUnchanged(path);
	mkdirSync(dirname(path), { recursive: true });
	let writeMode = mode ?? 0o600;
	if (mode === undefined && existsSync(path)) {
		const current = lstatSync(path);
		if (!current.isFile() || current.isSymbolicLink()) {
			throw new Error(`Refusing to atomically replace a non-regular setup file: ${path}`);
		}
		writeMode = current.mode & 0o777;
	}
	const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
	try {
		writeFileSync(temporary, contents, { flag: "wx", mode: writeMode, flush: true });
		if (process.platform !== "win32") chmodSync(temporary, writeMode);
		if (expected && !setupFileSnapshotUnchanged(expected)) {
			throw new Error(`Setup target changed during replacement: ${resolve(path)}`);
		}
		assertTrackedSetupFileUnchanged(path);
		if (exclusive || baseline?.contents === null) {
			linkSync(temporary, path);
			recordSetupFileMutation(path, contents, writeMode);
			unlinkSync(temporary);
		} else {
			renameSync(temporary, path);
			recordSetupFileMutation(path, contents, writeMode);
		}
		fsyncParentBestEffort(path);
	} catch (error) {
		try {
			unlinkSync(temporary);
		} catch {
			// The temporary may not exist or may already have been renamed.
		}
		throw error;
	}
}

export function atomicRemoveSetupFile(path: string): void {
	assertTrackedSetupFileUnchanged(path);
	if (!existsSync(path)) return;
	unlinkSync(path);
	recordSetupFileMutation(path, null, null);
	fsyncParentBestEffort(path);
}
