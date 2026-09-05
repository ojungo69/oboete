import { isDeepStrictEqual } from "node:util";

function nearestRankP95(values) {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.ceil(ordered.length * 0.95) - 1];
}

function intervalDuration(interval, label) {
  if (interval.endMonotonicMs < interval.startMonotonicMs) {
    throw new Error(`${label} timing interval is reversed`);
  }
  return interval.endMonotonicMs - interval.startMonotonicMs;
}

function thresholdsPass(aggregates, thresholds) {
  return (aggregates.captureP95Ms === null || aggregates.captureP95Ms < thresholds.captureP95Ms) &&
    (aggregates.warmInjectionP95Ms === null ||
      aggregates.warmInjectionP95Ms < thresholds.warmInjectionP95Ms) &&
    (aggregates.shortColdLexicalInjectionMs === null ||
      aggregates.shortColdLexicalInjectionMs < thresholds.shortColdLexicalInjectionMs);
}

function validateExceptionalLatency(result) {
  if (result.latencyEvidence.captureEventIds.length !== 0 ||
      result.latencyEvidence.runs.length !== 0 ||
      !Object.values(result.latencyEvidence.aggregates).every((value) => value === null)) {
    throw new Error("unsupported/not-run latency evidence is not empty");
  }
  return false;
}

function validateRunEvidence(runs, scenario, result, protocol, applicability) {
  const { expectedCaptureEventIds, warmInjectionApplies, coldInjectionApplies } = applicability;
  const expectedResetMode = result.resourceSampleMode === "cold"
    ? "fresh_isolated_data"
    : "fresh_namespace_on_ready_process";
  const expectedRunEventIds = (ordinal) => expectedCaptureEventIds.map(
    (eventId) => `${scenario.scenarioId}:run-${ordinal}:${eventId}`,
  );
  const runsMatch = runs.length === protocol.runsPerScenario && runs.every((run, index) =>
    run.runOrdinal === index + 1 &&
    run.discarded === (run.runOrdinal <= protocol.discardInitialRunsPerScenario) &&
    run.resetMode === expectedResetMode &&
    run.repositoryNamespace === `${scenario.scenarioId}:repo-${run.runOrdinal}` &&
    run.sessionNamespace === `${scenario.scenarioId}:session-${run.runOrdinal}` &&
    isDeepStrictEqual(run.captureTimings.map((timing) => timing.eventId),
      expectedRunEventIds(run.runOrdinal)) &&
    run.captureTimings.every((timing, timingIndex, timings) => timingIndex === 0 ||
      timing.startMonotonicMs >= timings[timingIndex - 1].endMonotonicMs) &&
    (warmInjectionApplies ? run.warmInjectionTiming !== null : run.warmInjectionTiming === null) &&
    (coldInjectionApplies
      ? run.coldLexicalInjectionTiming !== null
      : run.coldLexicalInjectionTiming === null) &&
    [run.warmInjectionTiming, run.coldLexicalInjectionTiming].filter(Boolean).every(
      (timing) => run.captureTimings.length === 0 ||
        timing.startMonotonicMs >= run.captureTimings.at(-1).endMonotonicMs,
    )
  );
  if (!runsMatch) {
    throw new Error("latency run evidence does not match the pinned sampling protocol");
  }
  for (const run of runs) {
    run.captureTimings.forEach((timing) => intervalDuration(timing, "capture"));
    if (run.warmInjectionTiming) intervalDuration(run.warmInjectionTiming, "warm injection");
    if (run.coldLexicalInjectionTiming) {
      intervalDuration(run.coldLexicalInjectionTiming, "cold lexical injection");
    }
  }
}

function deriveAggregates(measuredRuns, applicability) {
  const { captureApplies, warmInjectionApplies, coldInjectionApplies } = applicability;
  return {
    captureP95Ms: captureApplies
      ? nearestRankP95(measuredRuns.flatMap((run) =>
          run.captureTimings.map((timing) => intervalDuration(timing, "capture"))))
      : null,
    warmInjectionP95Ms: warmInjectionApplies
      ? nearestRankP95(measuredRuns.map((run) =>
          intervalDuration(run.warmInjectionTiming, "warm injection")))
      : null,
    shortColdLexicalInjectionMs: coldInjectionApplies
      ? nearestRankP95(measuredRuns.map((run) =>
          intervalDuration(run.coldLexicalInjectionTiming, "cold lexical injection")))
      : null,
  };
}

export function evaluateLatencyEvidence(result, scenario, fixture, exceptionalState, observedRuns) {
  const protocol = fixture.samplingProtocol;
  const metricApplies = (name) => protocol.metrics[name].scenarios.includes(scenario.scenarioId);
  const captureApplies = metricApplies("captureP95Ms");
  const warmInjectionApplies = metricApplies("warmInjectionP95Ms");
  const coldInjectionApplies = metricApplies("shortColdLexicalInjectionMs");
  const expectedCaptureEventIds = captureApplies ? scenario.events.map((event) => event.eventId) : [];
  if (exceptionalState) return validateExceptionalLatency(result);

  const runs = observedRuns;
  const applicability = {
    captureApplies, warmInjectionApplies, coldInjectionApplies, expectedCaptureEventIds,
  };
  if (!isDeepStrictEqual(result.latencyEvidence.captureEventIds, expectedCaptureEventIds)) {
    throw new Error("latency capture events do not match the pinned sampling protocol");
  }
  validateRunEvidence(runs, scenario, result, protocol, applicability);
  const measuredRuns = runs.filter((run) => !run.discarded);
  if (measuredRuns.length !== protocol.measuredRunsPerScenario) {
    throw new Error("latency measured-run count does not match the pinned protocol");
  }
  const measuredEventIds = measuredRuns.flatMap((run) =>
    run.captureTimings.map((timing) => timing.eventId));
  if (result.resourceSampleMode === "warm" &&
      (new Set(measuredRuns.map((run) => run.repositoryNamespace)).size !== measuredRuns.length ||
       new Set(measuredRuns.map((run) => run.sessionNamespace)).size !== measuredRuns.length ||
       new Set(measuredEventIds).size !== measuredEventIds.length)) {
    throw new Error("warm latency runs reused a repository, session, or event identity");
  }
  const expectedAggregates = deriveAggregates(measuredRuns, applicability);
  if (!isDeepStrictEqual(result.latencyEvidence.aggregates, expectedAggregates)) {
    throw new Error("latency aggregates do not match the recorded measured runs");
  }
  return thresholdsPass(expectedAggregates, fixture.thresholds);
}
