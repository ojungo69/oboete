import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { isDeepStrictEqual } from "node:util";

import { canonicalizeJson } from "../../../harness/schema/jcs.ts";
import { lineageDigest } from "./alpha-result-lineage.mjs";

const SENSITIVITY_RANK = { eligible: 0, local_only: 1, private: 2, secret: 3 };

export function tokenizeRenderPayload(payload) {
  const tokens = payload.match(/[\p{L}\p{N}_]+|[^\s]/gu) ?? [];
  return tokens.map((token) => createHash("sha256")
    .update("free-mem:fixture-token:v1\0")
    .update(token)
    .digest()
    .readUInt32BE(0));
}

function validateEvidence(evidence, renderedBytes, injectedTokens, label) {
  if (renderedBytes === 0 && injectedTokens === 0) {
    if (evidence !== null) throw new Error(`${label} render evidence is not empty`);
    return null;
  }
  if (evidence === null) throw new Error(`${label} render evidence is missing`);
  let parsedPayload;
  try {
    parsedPayload = JSON.parse(evidence.utf8Payload);
  } catch {
    throw new Error(`${label} render payload is not valid JSON`);
  }
  if (Buffer.byteLength(evidence.utf8Payload, "utf8") !== renderedBytes ||
      evidence.tokenIds.length !== injectedTokens ||
      evidence.utf8Payload !== canonicalizeJson(parsedPayload) ||
      !isDeepStrictEqual(evidence.tokenIds, tokenizeRenderPayload(evidence.utf8Payload))) {
    throw new Error(`${label} render aggregates do not match their exact evidence`);
  }
  return parsedPayload;
}

export function buildRenderPayload(result, scenario, fixture, items, packId) {
  const manifest = scenario.derivationManifestId
    ? fixture.localDerivationManifest
    : fixture.effectiveConfiguration;
  return {
    injectionPack: {
      packVersion: 1,
      packId,
      targetDestinationClass: result.targetDestinationClass,
      targetSessionId: scenario.targetSessionId,
      targetRepositoryScope: scenario.targetRepositoryScope,
      resolvedDestinationPolicy:
        fixture.effectiveConfiguration.destinationPolicyMap[result.targetDestinationClass],
      manifestIdentity: {
        manifestId: manifest.manifestId,
        effectiveManifestFingerprint: result.effectiveManifestFingerprint,
      },
      packDegradations: result.packDegradations,
      items,
    },
  };
}

function validateTraceProvenance(result, scenario) {
  const events = new Map(scenario.events.map((event) => [event.eventId, event]));
  const revisionContent = ({ reason: _reason, selectionReason: _selectionReason, ...item }) => item;
  const normalItems = [...result.injectedItems,
    ...result.omittedItems.filter((item) => item.reason !== "duplicate_revision")];
  for (const name of ["lineageId", "memoryId", "revisionId"]) {
    if (new Set(normalItems.map((item) => item[name])).size !== normalItems.length) {
      throw new Error("result trace contains duplicate active identities");
    }
  }
  const activeSpansByEvent = Map.groupBy(
    normalItems.flatMap((item) => item.sourceSpans), (span) => span.eventId,
  );
  for (const spans of activeSpansByEvent.values()) {
    spans.sort((left, right) => left.startByte - right.startByte);
    if (spans.some((span, index) => index > 0 && span.startByte < spans[index - 1].endByte)) {
      throw new Error("result trace contains overlapping active source anchors");
    }
  }
  if (!result.omittedItems.filter((item) => item.reason === "duplicate_revision").every(
    (duplicate) => result.injectedItems.some((item) =>
      isDeepStrictEqual(revisionContent(item), revisionContent(duplicate))),
  )) throw new Error("duplicate revision omission has no retained active item");
  for (const item of [...result.injectedItems, ...result.omittedItems]) {
    const sourceIds = [...new Set(item.sourceEventIds)].sort();
    const spanIds = [...new Set(item.sourceSpans.map((span) => span.eventId))].sort();
    const sourceSensitivities = sourceIds.map((id) => events.get(id)?.sensitivity);
    const derivedSensitivity = sourceSensitivities.length === 0 || sourceSensitivities.includes(undefined) ? null :
      sourceSensitivities.reduce((left, right) =>
        SENSITIVITY_RANK[left] >= SENSITIVITY_RANK[right] ? left : right);
    const spansPass = new Set(item.sourceSpans.map((span) => canonicalizeJson(span))).size ===
      item.sourceSpans.length && item.sourceSpans.every((span) => {
      const event = events.get(span.eventId);
      const bytes = event && Buffer.from(event.redactedPayload, "utf8");
      return bytes && span.startByte >= 0 && span.startByte < span.endByte &&
        span.endByte <= bytes.length &&
        (span.startByte === 0 || (bytes[span.startByte] & 0xc0) !== 0x80) &&
        (span.endByte === bytes.length || (bytes[span.endByte] & 0xc0) !== 0x80);
    });
    if (item.sensitivity !== derivedSensitivity ||
        sourceIds.length !== item.sourceEventIds.length || !isDeepStrictEqual(sourceIds, spanIds) ||
        sourceIds.some((id) => !events.has(id)) ||
        !spansPass || item.lineageId !== lineageDigest(scenario.sourceRepositoryScope, item.sourceSpans)) {
      throw new Error("result trace provenance does not match scenario source events");
    }
  }
}

function validateAttemptedItems(result, attemptedItems, scenario) {
  validateTraceProvenance(result, scenario);
  const allowedOmissionReasons = new Set(["duplicate_revision", "omitted_budget", "omitted_ineligible"]);
  if (result.omittedItems.some((item) => !allowedOmissionReasons.has(item.reason))) {
    throw new Error("result uses an omission reason outside the Slice 1 contract");
  }
  if (result.attemptedItems === "same_as_final") {
    if (result.omittedItems.some((item) => item.reason === "omitted_budget")) {
      throw new Error("pruned render candidates require explicit attempted items");
    }
    return;
  }
  const pruned = result.omittedItems.filter((item) => item.reason === "omitted_budget")
    .map(({ reason: _reason, ...item }) => ({ ...item, selectionReason: item.sourceLane }));
  if (!isDeepStrictEqual(attemptedItems, [...result.injectedItems, ...pruned])) {
    throw new Error("attempted render does not match the ordered traced candidates");
  }
}

export function validateRenderEvidence(result, scenario, fixture, finalPackExpected) {
  if ((result.attemptedItems === "same_as_final" ||
      result.attemptedRenderEvidence === "same_as_final") && result.finalRenderEvidence === null) {
    throw new Error("attempted render aliases require an observed final pack");
  }
  const attemptedItems = result.attemptedItems === "same_as_final"
    ? result.injectedItems
    : result.attemptedItems;
  validateAttemptedItems(result, attemptedItems, scenario);
  const attemptedEvidence = result.attemptedRenderEvidence === "same_as_final"
    ? result.finalRenderEvidence
    : result.attemptedRenderEvidence;
  const attemptedBoundaryObserved = scenario.drainCondition.targetInjectionAcknowledged &&
    result.milestones.some((item) => item.name === "target_selection_finished");
  if (!attemptedBoundaryObserved &&
      (attemptedItems.length !== 0 || attemptedEvidence !== null ||
        result.attemptedRenderedBytes !== 0 || result.attemptedInjectedTokens !== 0)) {
    throw new Error("attempted render exists without an observed selection boundary");
  }
  const attemptedPayload = validateEvidence(
    attemptedEvidence,
    result.attemptedRenderedBytes,
    result.attemptedInjectedTokens,
    "attempted",
  );
  const attemptedPackId = result.attemptedRenderEvidence === "same_as_final"
    ? result.packId
    : null;
  if (attemptedPayload !== null && !isDeepStrictEqual(attemptedPayload,
    buildRenderPayload(result, scenario, fixture, attemptedItems, attemptedPackId))) {
    throw new Error("attempted render payload does not match attempted items");
  }
  const envelope = fixture.effectiveConfiguration.resourceProfile.injectionEnvelope;
  if (!finalPackExpected && (result.attemptedRenderedBytes > envelope.maxRenderedBytes ||
      result.attemptedInjectedTokens > envelope.maxInjectedTokens)) {
    throw new Error("oversized attempted InjectionPack has no valid final output");
  }
  const finalPayload = validateEvidence(
    result.finalRenderEvidence,
    result.renderedBytes,
    result.injectedTokens,
    "final",
  );
  if ((finalPayload !== null) !== finalPackExpected ||
      (finalPayload === null) !== (result.packId === null) ||
      (finalPayload !== null && !isDeepStrictEqual(finalPayload,
        buildRenderPayload(result, scenario, fixture, result.injectedItems, result.packId)))) {
    throw new Error("final render payload does not match delivered items");
  }
}
