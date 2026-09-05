export function clearProviderEgressEvidence(evidence, caseId) {
  const record = evidence.scenarios.find((item) => item.caseId === caseId);
  if (!record) throw new Error(`test runner evidence case is missing: ${caseId}`);
  // A projection intentionally clears the shared observed receipt on its base scenario clone.
  const projectedSource = record.providerEgressEvidence.kind === "projection"
    ? evidence.scenarios.find((item) =>
      item.caseId === record.providerEgressEvidence.sourceCaseId)
    : null;
  if (record.providerEgressEvidence.kind === "projection" && !projectedSource) {
    throw new Error(`test provider egress projection source is missing: ${caseId}`);
  }
  const raw = record.providerEgressEvidence.kind === "observed"
    ? record.providerEgressEvidence
    : projectedSource.providerEgressEvidence;
  Object.assign(raw, {
    authorization: null,
    firstProviderRequestStartedMonotonicMs: null,
    lastProviderRequestFinishedMonotonicMs: null,
    providerRequestCount: 0,
    providerPayloadCount: 0,
    credentialBytesSent: 0,
    payloadBytesSent: 0,
    restrictedPayloadBytesSent: 0,
    forbiddenSentinelObservationCount: 0,
    sourcePayloadBytesBySensitivity: { eligible: 0, localOnly: 0, private: 0, secret: 0 },
    redirectLocationRequestCount: 0,
    redirectLocationPayloadBytesSent: 0,
    resentPayloadCount: 0,
  });
}
