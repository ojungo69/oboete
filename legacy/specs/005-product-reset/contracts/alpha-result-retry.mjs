import { isDeepStrictEqual } from "node:util";

export function expectedRetryEvidence(scenario) {
  const observedRetryCase = (item) => {
    const providerAttempted = item.expected.attemptDelta > 0;
    const observedDurableOutput = item.providerOutcome === "valid"
      ? (scenario.fault.recoveredOutput ?? scenario.summaryProviderStub)
      : null;
    return {
      caseId: item.caseId,
      deliveredSignals: item.signals,
      consumedSignalIds: item.expectedConsumedSignalIds,
      ignoredSignalIds: item.expectedIgnoredSignalIds,
      providerAttempted,
      observedTransmissionEvidence: item.expectedTransmissionEvidence,
      observedProviderOutcome: providerAttempted ? item.providerOutcome : null,
      observedDurableOutput,
      observedTransition: item.expected,
    };
  };
  if (scenario.fault?.resumeCases) {
    return {
      observedInitialSnapshot: scenario.fault.resumeCaseInitialSnapshot,
      cases: scenario.fault.resumeCases.map(observedRetryCase),
    };
  }
  if (scenario.fault?.redirectRecovery) {
    const recovery = scenario.fault.redirectRecovery;
    return {
      observedInitialSnapshot: scenario.fault.resumeCaseInitialSnapshot,
      redirectCase: observedRetryCase({
        caseId: recovery.caseId,
        signals: [recovery.signal],
        expectedConsumedSignalIds: recovery.expectedConsumedSignalIds,
        expectedIgnoredSignalIds: recovery.expectedIgnoredSignalIds,
        providerOutcome: recovery.providerOutcome,
        expectedTransmissionEvidence: recovery.expectedTransmissionEvidence,
        expected: recovery.expected,
      }),
      recoveryTransportEvidence: {
        rejectedLocationRequestCount: recovery.oldLocationRequestCountAfterActivation,
        rejectedLocationPayloadBytesSent: recovery.oldLocationPayloadBytesSentAfterActivation,
        resentPayloadCount: recovery.resentPayloadCountAfterActivation,
      },
    };
  }
  return null;
}

export function assertRetryEvidenceConsistent(result) {
  const observedCases = result.retryEvidence?.cases ??
    (result.retryEvidence?.redirectCase ? [result.retryEvidence.redirectCase] : []);
  if (!observedCases.every((item) =>
    item.providerAttempted === (item.observedTransition.attemptDelta > 0) &&
    item.observedTransmissionEvidence.remoteProviderRequestCount ===
      item.observedTransition.attemptDelta &&
    item.observedTransmissionEvidence.remoteProviderPayloadCount ===
      item.observedTransmissionEvidence.remoteProviderRequestCount &&
    (item.providerAttempted
      ? item.observedTransmissionEvidence.credentialBytesSent > 0 &&
        item.observedTransmissionEvidence.payloadBytesSent > 0
      : Object.values(item.observedTransmissionEvidence).every((value) => value === 0)) &&
    item.ignoredSignalIds.length === item.observedTransition.ignoredSignalCount &&
    isDeepStrictEqual(
      item.deliveredSignals.map((signal) => signal.signalId).sort(),
      [...item.consumedSignalIds, ...item.ignoredSignalIds].sort(),
    ) &&
    item.observedTransition.durableMemoryCount === (item.observedDurableOutput
      ? Number(Object.hasOwn(item.observedDurableOutput, "summary")) +
        item.observedDurableOutput.memoryItems.length
      : 0) &&
    (item.providerAttempted
      ? item.observedProviderOutcome !== null &&
        ((item.observedProviderOutcome === "valid") === (item.observedDurableOutput !== null))
      : item.observedProviderOutcome === null && item.observedDurableOutput === null)
  )) {
    throw new Error("observed retry evidence is internally inconsistent");
  }
}
