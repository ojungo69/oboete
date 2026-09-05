import { Buffer } from "node:buffer";
import { closeSync, constants, fstatSync, openSync, readSync, realpathSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";

import { decodeUtf8, parseIJson } from "../../../harness/schema/jcs.ts";

export const isWithin = (base, target) => {
  const path = relative(base, target);
  return path === "" || (!isAbsolute(path) && path !== ".." && !path.startsWith(`..${sep}`));
};

function readBoundedIJson(descriptor, maxBytes,
  { requireRegularFile = false, requireRunnerOwnership = false } = {}) {
  const before = fstatSync(descriptor, { bigint: true });
  if (requireRegularFile && !before.isFile()) {
    throw new Error("result input path is not a regular file");
  }
  if (requireRunnerOwnership && ((before.mode & 0o022n) !== 0n ||
      (typeof process.getuid === "function" && before.uid !== BigInt(process.getuid())))) {
    throw new Error("runner evidence file is not runner-owned and immutable");
  }
  if (before.isFile() && before.size > BigInt(maxBytes)) {
    throw new Error("result input exceeds the fixed byte limit");
  }
  const chunks = [];
  const buffer = Buffer.allocUnsafe(64 * 1024);
  let bytesRead = 0;
  for (let size; (size = readSync(descriptor, buffer, 0, buffer.length, null)) > 0;) {
    bytesRead += size;
    if (bytesRead > maxBytes) throw new Error("result input exceeds the fixed byte limit");
    chunks.push(Buffer.from(buffer.subarray(0, size)));
  }
  const after = fstatSync(descriptor, { bigint: true });
  if (before.isFile() && (BigInt(bytesRead) !== after.size ||
      ["dev", "ino", "size", "mtimeNs", "ctimeNs"].some((name) => before[name] !== after[name]))) {
    throw new Error("result input changed while it was being read");
  }
  const bytes = Buffer.concat(chunks, bytesRead);
  return parseIJson(decodeUtf8(bytes, "candidate result"));
}

export function readBoundedIJsonFile(path, maxBytes, root = null) {
  const descriptor = openSync(path, constants.O_RDONLY | constants.O_NONBLOCK | constants.O_NOFOLLOW);
  try {
    if (root !== null && !isWithin(realpathSync(resolve(root)),
      realpathSync(`/proc/self/fd/${descriptor}`))) {
      throw new Error("result input path escapes its runner-owned root");
    }
    return readBoundedIJson(descriptor, maxBytes, {
      requireRegularFile: true,
      requireRunnerOwnership: root !== null,
    });
  } finally {
    closeSync(descriptor);
  }
}

export function readBoundedIJsonStdin(maxBytes) {
  return readBoundedIJson(0, maxBytes);
}
