export function validateSelectionTiming(result, exceptionalState, expectedInputCandidates,
  selectionTimeBudgetMs) {
  const evidence = result.selectionTimingEvidence;
  if (exceptionalState) {
    if (evidence !== null || result.selectionElapsedMs !== 0) {
      throw new Error("unsupported/not-run selection timing evidence is not empty");
    }
    return false;
  }
  const start = result.milestones.find((item) => item.name === "target_selection_started");
  const end = result.milestones.find((item) => item.name === "target_selection_finished");
  const candidateCounts = ["inputCandidates", "tracedCandidates", "deadlineUnprocessed",
    "admittedCandidates", "selectedItems"];
  if (result.drain.timedOut && start && !end) {
    if (evidence !== null || result.selectionElapsedMs !== 0 ||
        !candidateCounts.every((name) => result.counts[name] === 0)) {
      throw new Error("partial timed-out selection has completed timing evidence");
    }
    return false;
  }
  if (!start && !end) {
    if (evidence !== null || result.selectionElapsedMs !== 0 ||
        !candidateCounts.every((name) => result.counts[name] === 0)) {
      throw new Error("selection timing exists without an observed selection lifecycle");
    }
    return false;
  }
  if (!start || !end || evidence === null ||
      evidence.startMonotonicMs !== start.monotonicMs ||
      evidence.endMonotonicMs !== end.monotonicMs ||
      evidence.endMonotonicMs < evidence.startMonotonicMs ||
      result.selectionElapsedMs !== evidence.endMonotonicMs - evidence.startMonotonicMs) {
    throw new Error("selection elapsed time does not match monotonic timing evidence");
  }
  if (result.counts.inputCandidates !== expectedInputCandidates) {
    throw new Error("completed selection input count does not match the scenario");
  }
  if (result.counts.deadlineUnprocessed > 0 &&
      result.selectionElapsedMs < selectionTimeBudgetMs) {
    throw new Error("selection left unprocessed candidates before its deadline");
  }
  return true;
}
