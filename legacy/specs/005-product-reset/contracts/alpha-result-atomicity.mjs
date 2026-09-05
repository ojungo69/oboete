import { isDeepStrictEqual } from "node:util";

export function validateOutputLimitAtomicity(result, scenario, exceptionalState) {
  const expected = scenario.fault?.kind === "summary_provider_output_limit_exceeded"
    ? scenario.fault.atomicityEvidence
    : null;
  if (expected === null || exceptionalState) {
    if (result.outputLimitAtomicityEvidence !== null) {
      throw new Error("output-limit atomicity evidence exists outside its scenario");
    }
    return;
  }
  if (result.drain.timedOut) {
    if (result.outputLimitAtomicityEvidence !== null) {
      throw new Error("timed-out output-limit result has unobserved atomicity evidence");
    }
    return;
  }
  const times = new Map(result.milestones.map((item) => [item.name, item.monotonicMs]));
  const start = times.get(expected.observationStartMilestone);
  const end = times.get(expected.observationEndMilestone);
  const evidence = result.outputLimitAtomicityEvidence;
  if (!isDeepStrictEqual(evidence, expected)) {
    throw new Error("output-limit atomicity evidence does not match the pinned oracle");
  }
  const names = result.milestones.map((item) => item.name);
  const startIndex = names.indexOf(expected.observationStartMilestone);
  const endIndex = names.indexOf(expected.observationEndMilestone);
  const receiptIds = evidence.writerReceipts.map((receipt) => receipt.receiptId);
  const receiptsPass = receiptIds.length === new Set(receiptIds).size &&
    evidence.writerReceipts.every((receipt) => receipt.jobId === expected.jobId &&
      receipt.attemptedDerivedItemCount === expected.observedResultCount) &&
    evidence.committedDerivedBatchCount === evidence.writerReceipts.filter((receipt) =>
      receipt.committedDerivedItemCount > 0 || receipt.committedMutationCount > 0).length &&
    evidence.committedDerivedItemMutationCount === evidence.writerReceipts.reduce(
      (count, receipt) => count + receipt.committedMutationCount, 0);
  const samplesPass = isDeepStrictEqual(
    evidence.durableObserverSamples.map((sample) => sample.milestone),
    names.slice(startIndex, endIndex + 1),
  ) && evidence.maximumObservableDerivedItemCount === Math.max(
    ...evidence.durableObserverSamples.map((sample) => sample.observableDerivedItemCount),
  ) && evidence.forbiddenSentinelObservationCount === evidence.durableObserverSamples.filter(
    (sample) => sample.forbiddenSentinelObserved,
  ).length;
  if (typeof start !== "number" || typeof end !== "number" || end <= start ||
      startIndex < 0 || endIndex <= startIndex || !receiptsPass || !samplesPass) {
    throw new Error("output-limit atomicity evidence does not cover the pinned processing window");
  }
}
