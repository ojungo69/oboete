import { createHash } from "node:crypto";
import { closeSync, constants, fstatSync, opendirSync, openSync, readSync,
  realpathSync, statSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";

import { canonicalizeJson } from "../../../harness/schema/jcs.ts";
import { isWithin } from "./alpha-result-input.mjs";

const isRunnerOwnedImmutable = (stats) => (stats.mode & 0o022n) === 0n &&
  (typeof process.getuid !== "function" || stats.uid === BigInt(process.getuid()));

function artifactFiles(root, directory, limits, state = { entries: 0, files: 0 }, depth = 0) {
  if (depth > limits.maxDirectoryDepth) throw new Error("candidate artifact exceeds depth limit");
  const directoryStats = statSync(directory, { bigint: true });
  if (!directoryStats.isDirectory() || !isRunnerOwnedImmutable(directoryStats)) {
    throw new Error(`candidate artifact directory is not runner-owned and immutable: ${directory}`);
  }
  const files = [];
  const opened = opendirSync(directory);
  try {
    for (let entry; (entry = opened.readSync()) !== null;) {
      state.entries += 1;
      if (state.entries > limits.maxEntryCount) throw new Error("candidate artifact exceeds entry limit");
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        const nested = artifactFiles(root, path, limits, state, depth + 1);
        if (nested.length === 0) throw new Error(`candidate artifact contains an empty directory: ${path}`);
        files.push(...nested);
      } else {
        if (!entry.isFile()) throw new Error(`candidate artifact contains a non-regular entry: ${path}`);
        state.files += 1;
        if (state.files > limits.maxFileCount) throw new Error("candidate artifact exceeds file limit");
        files.push(relative(root, path).split(sep).join("/"));
      }
    }
  } finally {
    opened.closeSync();
  }
  return files.sort();
}

function hashArtifact(path, candidateRoot, maxFileBytes) {
  const descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const before = fstatSync(descriptor, { bigint: true });
    const openedPath = realpathSync(`/proc/self/fd/${descriptor}`);
    if (!before.isFile() || !isWithin(candidateRoot, openedPath)) {
      throw new Error(`artifact path is not a contained regular file: ${path}`);
    }
    if (!isRunnerOwnedImmutable(before)) {
      throw new Error(`candidate artifact file is not runner-owned and immutable: ${path}`);
    }
    if (before.size > BigInt(maxFileBytes)) throw new Error(`artifact file exceeds limit: ${path}`);
    const hash = createHash("sha256");
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let bytesRead = 0;
    for (let size; (size = readSync(descriptor, buffer, 0, buffer.length, null)) > 0;) {
      hash.update(buffer.subarray(0, size));
      bytesRead += size;
      if (bytesRead > maxFileBytes) throw new Error(`artifact file exceeds limit: ${path}`);
    }
    const after = fstatSync(descriptor, { bigint: true });
    if (BigInt(bytesRead) !== after.size ||
        ["dev", "ino", "size", "mtimeNs", "ctimeNs"].some((name) => before[name] !== after[name])) {
      throw new Error(`artifact changed while it was being hashed: ${path}`);
    }
    return { digest: `sha256:${hash.digest("hex")}`, size: bytesRead };
  } finally {
    closeSync(descriptor);
  }
}

function validateArtifactFiles(artifactRoot, candidateId, manifest, limits) {
  if (!/^(?!\.{1,2}$)[A-Za-z0-9][A-Za-z0-9._-]*$/.test(candidateId)) {
    throw new Error("candidate identifier is not a single safe path segment");
  }
  const manifestPaths = manifest.files.map((file) => file.path);
  if (!manifestPaths.every((path, index) => index === 0 || manifestPaths[index - 1] < path)) {
    throw new Error("artifact manifest paths are duplicated or not canonically sorted");
  }
  if (!manifestPaths.includes(manifest.entrypoint)) {
    throw new Error("candidate entrypoint is not present in the artifact manifest");
  }
  const root = realpathSync(resolve(artifactRoot));
  if (!isRunnerOwnedImmutable(statSync(root, { bigint: true }))) {
    throw new Error("candidate artifact root is not runner-owned and immutable");
  }
  const candidateRoot = realpathSync(resolve(root, candidateId));
  if (!isWithin(root, candidateRoot)) {
    throw new Error("candidate artifact directory escapes the artifact root");
  }
  const actualPaths = artifactFiles(candidateRoot, candidateRoot, limits);
  if (actualPaths.length > limits.maxFileCount || actualPaths.length !== manifestPaths.length ||
      actualPaths.some((path, index) => path !== manifestPaths[index])) {
    throw new Error("candidate artifact files do not exactly match the manifest");
  }
  let totalBytes = 0;
  for (const file of manifest.files) {
    const path = realpathSync(resolve(candidateRoot, file.path));
    if (!isWithin(candidateRoot, path)) throw new Error(`artifact path escapes root: ${file.path}`);
    const remainingBytes = Math.min(
      limits.maxFileBytes,
      limits.maxTotalBytes - totalBytes,
    );
    const hashed = hashArtifact(path, candidateRoot, remainingBytes);
    totalBytes += hashed.size;
    if (totalBytes > limits.maxTotalBytes) throw new Error("candidate artifact exceeds total limit");
    if (hashed.digest !== file.sha256) {
      throw new Error(`artifact bytes do not match manifest digest: ${file.path}`);
    }
  }
}

export function validateArtifact(result, artifactRoot, baseCommit, limits) {
  const metadata = result.artifactMetadata;
  const fingerprint = (domain, value) => `sha256:${createHash("sha256")
    .update(domain).update(canonicalizeJson(value)).digest("hex")}`;
  if (metadata.candidateId !== result.candidateId || metadata.baseCommit !== baseCommit ||
      metadata.contentSha256 !== fingerprint(
        "free-mem:alpha-artifact-content:v1\0", metadata.manifest,
      ) || result.artifactFingerprint !== fingerprint(
        "free-mem:alpha-candidate-artifact:v1\0", metadata,
      )) {
    throw new Error("candidate artifact identity does not match its manifest");
  }
  validateArtifactFiles(artifactRoot, result.candidateId, metadata.manifest, limits);
}
