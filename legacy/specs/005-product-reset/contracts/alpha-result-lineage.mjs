import { createHash } from "node:crypto";

import { canonicalizeJson } from "../../../harness/schema/jcs.ts";

export function lineageDigest(repositoryScope, sourceSpans) {
  const normalizedSpans = [
    ...new Map(sourceSpans.map((span) => [canonicalizeJson(span), span])).values(),
  ].sort(
    (left, right) =>
      (left.eventId < right.eventId ? -1 : left.eventId > right.eventId ? 1 : 0) ||
      left.startByte - right.startByte || left.endByte - right.endByte,
  );
  return createHash("sha256")
    .update("free-mem:memory-lineage:v1\0")
    .update(canonicalizeJson({ repositoryScope, sourceSpans: normalizedSpans }))
    .digest("hex");
}
