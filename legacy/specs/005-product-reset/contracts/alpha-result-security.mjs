import { createHash } from "node:crypto";
import { isIP } from "node:net";
import { isDeepStrictEqual } from "node:util";
import { URL } from "node:url";

import { canonicalizeJson } from "../../../harness/schema/jcs.ts";

const ORACLE_EVIDENCE_FIELDS = [
  "remoteProviderRequestCount",
  "remoteProviderPayloadCount",
  "remoteInjectionCount",
  "persistedSecretCount",
  "injectedSecretCount",
  "credentialBytesSent",
  "payloadBytesSent",
  "restrictedPayloadBytesSent",
  "redirectLocationRequestCount",
  "redirectLocationPayloadBytesSent",
  "resentPayloadCount",
];

function validateTlsPreflightReceipt(receipt, endpoints, publicCaSha256) {
  const endpoint = endpoints.find((item) => item.hostname === receipt.hostname);
  const hostname = endpoint?.hostname;
  const address = hostname?.startsWith("[") ? hostname.slice(1, -1) : hostname;
  const expectedSni = isIP(address ?? "") ? null : hostname;
  if (receipt.sni !== expectedSni) {
    throw new Error("TLS preflight SNI does not match its hostname");
  }
  const expectedPort = Number(endpoint?.port || 443);
  if (receipt.port !== expectedPort) {
    throw new Error("TLS preflight port does not match its endpoint");
  }
  if (receipt.timeoutMs !== 5000) {
    throw new Error("TLS preflight timeout is not 5000 ms");
  }
  if (receipt.endMonotonicMs < receipt.startMonotonicMs) {
    throw new Error("TLS preflight monotonic interval is reversed");
  }
  if (receipt.endMonotonicMs - receipt.startMonotonicMs > receipt.timeoutMs) {
    throw new Error("TLS preflight duration exceeds its timeout");
  }
  if (receipt.result !== "verified") {
    throw new Error("TLS preflight result is not verified");
  }
  if (!receipt.chainValidation) {
    throw new Error("TLS preflight disabled chain validation");
  }
  if (!receipt.hostnameValidation) {
    throw new Error("TLS preflight disabled hostname validation");
  }
  if (receipt.trustAnchorSha256 !== publicCaSha256) {
    throw new Error("TLS preflight trust anchor does not match the runner public CA");
  }
  if (!/^sha256:[0-9a-f]{64}$/u.test(receipt.peerCertificateSha256) ||
      receipt.peerCertificateSha256 === receipt.trustAnchorSha256) {
    throw new Error("TLS preflight peer certificate fingerprint is invalid");
  }
  if (receipt.credentialBytesSent !== 0) {
    throw new Error("TLS preflight sent credential bytes");
  }
  if (receipt.payloadBytesSent !== 0) {
    throw new Error("TLS preflight sent payload bytes");
  }
  if (receipt.httpRequestCount !== 0) {
    throw new Error("TLS preflight sent an HTTP request");
  }
}

function validateTlsPreflightReceipts(receipts, fixture, publicCaSha256) {
  if (!Array.isArray(receipts) || receipts.length !== 6) {
    throw new Error("network trust evidence must contain exactly six TLS preflight receipts");
  }
  const receiptIds = receipts.map((receipt) => receipt.receiptId);
  if (!receiptIds.every((receiptId) =>
    typeof receiptId === "string" && /^[A-Za-z0-9][A-Za-z0-9._:-]*$/u.test(receiptId)
  )) {
    throw new Error("TLS preflight receipt IDs are not path-free opaque identifiers");
  }
  if (new Set(receiptIds).size !== receiptIds.length) {
    throw new Error("TLS preflight receipt identities are not unique");
  }
  const endpoints = [
    fixture.effectiveConfiguration.summaryProvider.endpointUrl,
    fixture.localDerivationManifest.summaryProvider.endpointUrl,
    fixture.repairedRemoteManifest.summaryProvider.endpointUrl,
  ].map((value) => new URL(value));
  const expectedPairs = endpoints.map((endpoint) => endpoint.hostname)
    .flatMap((hostname) => ["setup_activation", "daemon_start"]
      .map((phase) => `${hostname}\0${phase}`)).sort();
  const actualPairs = receipts.map((receipt) =>
    `${receipt.hostname}\0${receipt.phase}`).sort();
  if (!isDeepStrictEqual(actualPairs, expectedPairs)) {
    throw new Error("TLS preflight receipts do not cover the exact hostname/phase pair set");
  }
  for (const receipt of receipts) {
    validateTlsPreflightReceipt(receipt, endpoints, publicCaSha256);
  }
  const peersByEndpoint = new Map();
  const receiptsByEndpoint = new Map();
  for (const receipt of receipts) {
    const key = `${receipt.hostname}\0${receipt.port}`;
    const peers = peersByEndpoint.get(key) ?? new Set();
    peers.add(receipt.peerCertificateSha256);
    peersByEndpoint.set(key, peers);
    receiptsByEndpoint.set(key, [...(receiptsByEndpoint.get(key) ?? []), receipt]);
  }
  if ([...peersByEndpoint.values()].some((peers) => peers.size !== 1)) {
    throw new Error("TLS preflight peer certificate drifted between lifecycle phases");
  }
  if ([...receiptsByEndpoint.values()].some((pair) =>
    pair.find((item) => item.phase === "setup_activation").endMonotonicMs >=
      pair.find((item) => item.phase === "daemon_start").startMonotonicMs
  )) {
    throw new Error("TLS setup preflight did not finish before the daemon-start preflight");
  }
}

export function validateNetworkTrustEvidence(evidence, fixture, runnerInvocationId = null) {
  if (
    evidence?.version !== 1 ||
    evidence.baseHostname !== new URL(
      fixture.effectiveConfiguration.summaryProvider.endpointUrl,
    ).hostname ||
    evidence.localHostname !== new URL(
      fixture.localDerivationManifest.summaryProvider.endpointUrl,
    ).hostname ||
    evidence.repairedHostname !== new URL(
      fixture.repairedRemoteManifest.summaryProvider.endpointUrl,
    ).hostname ||
    !/^sha256:[0-9a-f]{64}$/u.test(evidence.publicCaSha256)
  ) {
    throw new Error("network trust evidence does not bind the fixed hostnames and public CA");
  }
  if ((runnerInvocationId !== null && evidence.runnerInvocationId !== runnerInvocationId) ||
      !evidence.tlsPreflightReceipts.every((receipt) =>
        receipt.runnerInvocationId === evidence.runnerInvocationId)) {
    throw new Error("network trust evidence does not match the runner invocation");
  }
  if (!evidence.chainValidation) {
    throw new Error("network trust evidence did not retain chain validation");
  }
  if (!evidence.hostnameValidation) {
    throw new Error("network trust evidence did not retain hostname validation");
  }
  if (evidence.privateKeyCommitted) {
    throw new Error("network trust evidence committed a private key");
  }
  validateTlsPreflightReceipts(
    evidence.tlsPreflightReceipts, fixture, evidence.publicCaSha256,
  );
}

const fingerprint = (domain, value) => `sha256:${createHash("sha256")
  .update(domain).update(canonicalizeJson(value)).digest("hex")}`;
const payloadDigest = (payload) => fingerprint("free-mem:event-payload-digest:v1\0", payload);

export function providerEgressCommittedEventSetFingerprint(events, repositoryScope) {
  return fingerprint("free-mem:provider-egress-committed-events:v1\0", events.map((event) => ({
    eventId: event.eventId,
    repositoryScope,
    payloadDigestVersion: event.payloadDigestVersion,
    payloadDigest: payloadDigest(event.redactedPayload),
    sensitivity: event.sensitivity,
    captureState: "accepted",
  })));
}

function expectedProviderSourcePayloadBytes(provider, committedEvents, providerRequestCount) {
  const allowedSensitivities = provider.executionLocation === "remote" ||
      provider.tlsPolicy !== "system"
    ? new Set(["eligible"])
    : new Set(["eligible", "local_only", "private"]);
  const sourcePayloadBytesBySensitivity = {
    eligible: 0, localOnly: 0, private: 0, secret: 0,
  };
  const outputKeys = {
    eligible: "eligible", local_only: "localOnly", private: "private", secret: "secret",
  };
  for (const event of committedEvents) {
    if (allowedSensitivities.has(event.sensitivity) && providerRequestCount > 0) {
      sourcePayloadBytesBySensitivity[outputKeys[event.sensitivity]] +=
        Buffer.byteLength(event.redactedPayload, "utf8") * providerRequestCount;
    }
  }
  return sourcePayloadBytesBySensitivity;
}

function expectedProviderEgressObservation(result, fixture, committedEventIds, {
  manifestFingerprint = result.effectiveManifestFingerprint,
  wireEvidence = result.securityEvidence,
} = {}) {
  const scenario = fixture.scenarios.find((item) => item.scenarioId === result.scenarioId);
  const manifests = [fixture.effectiveConfiguration, fixture.localDerivationManifest,
    fixture.repairedRemoteManifest, fixture.outputLimitRecoveryManifest];
  const manifest = manifests.find((item) =>
    item.configurationFingerprint === manifestFingerprint);
  if (!scenario || !manifest) {
    throw new Error("provider egress evidence cannot resolve scenario or manifest");
  }
  const provider = manifest.summaryProvider;
  const providerRequestCount = provider.executionLocation === "remote"
    ? wireEvidence.remoteProviderRequestCount
    : Number(scenario.providerTransmissionOracle.payloadBytesSent > 0);
  const providerPayloadCount = provider.executionLocation === "remote"
    ? wireEvidence.remoteProviderPayloadCount
    : providerRequestCount;
  const eventsById = new Map(scenario.events.map((event, index) =>
    [event.eventId, { event, index }]));
  const resolvedEvents = committedEventIds.map((eventId) => eventsById.get(eventId));
  if (providerRequestCount > 0 &&
      (committedEventIds.length !== result.counts.committed ||
        new Set(committedEventIds).size !== committedEventIds.length ||
        resolvedEvents.some((item) => !item) ||
        resolvedEvents.some((item, index) => index > 0 &&
          resolvedEvents[index - 1].index >= item.index))) {
    throw new Error("provider egress authorization event identities are incomplete or noncanonical");
  }
  const committedEvents = providerRequestCount > 0
    ? resolvedEvents.map((item) => item.event) : [];
  return {
    scenario,
    provider,
    providerRequestCount,
    providerPayloadCount,
    wireEvidence,
    committedEvents,
    sourcePayloadBytesBySensitivity: expectedProviderSourcePayloadBytes(
      provider, committedEvents, providerRequestCount,
    ),
  };
}

function validateProviderEgressClaims(evidence, expected) {
  const { provider, providerRequestCount, providerPayloadCount,
    sourcePayloadBytesBySensitivity, wireEvidence } = expected;
  if (evidence?.kind !== "observed" ||
      evidence.evidenceSource !== "runner_network_gate_and_stub_v1" ||
      evidence.providerFingerprint !== provider.providerFingerprint ||
      evidence.executionLocation !== provider.executionLocation) {
    throw new Error("provider egress evidence does not bind the active provider");
  }
  if (
    evidence.providerRequestCount !== providerRequestCount ||
    evidence.providerPayloadCount !== providerPayloadCount ||
    evidence.credentialBytesSent !== wireEvidence.credentialBytesSent ||
    evidence.payloadBytesSent !== wireEvidence.payloadBytesSent ||
    evidence.restrictedPayloadBytesSent !== wireEvidence.restrictedPayloadBytesSent ||
    evidence.forbiddenSentinelObservationCount !== wireEvidence.forbiddenSentinelObservationCount ||
    evidence.redirectLocationRequestCount !== wireEvidence.redirectLocationRequestCount ||
    evidence.redirectLocationPayloadBytesSent !== wireEvidence.redirectLocationPayloadBytesSent ||
    evidence.resentPayloadCount !== wireEvidence.resentPayloadCount ||
    evidence.preAuthorizationProviderAttemptCount !== 0 ||
    evidence.nonLoopbackSocketAttemptCount !== 0
  ) {
    throw new Error("provider egress evidence does not match observed wire aggregates");
  }
  const observedSourceBytes = evidence.sourcePayloadBytesBySensitivity;
  const observedSourceByteTotal = Object.values(observedSourceBytes)
    .reduce((sum, value) => sum + value, 0);
  if (observedSourceByteTotal > evidence.payloadBytesSent) {
    throw new Error("provider source sensitivity bytes exceed the provider payload");
  }
  if (evidence.restrictedPayloadBytesSent !== 0) {
    throw new Error("runner observed restricted provider payload");
  }
  if (evidence.forbiddenSentinelObservationCount !== 0) {
    throw new Error("runner observed a forbidden sentinel");
  }
  if (!isDeepStrictEqual(observedSourceBytes, sourcePayloadBytesBySensitivity) ||
      observedSourceBytes.secret !== 0 ||
      (provider.executionLocation === "remote" &&
        (observedSourceBytes.localOnly !== 0 || observedSourceBytes.private !== 0)) ||
      (provider.executionLocation === "local" && provider.tlsPolicy !== "system" &&
        (evidence.credentialBytesSent !== 0 || observedSourceBytes.localOnly !== 0 ||
          observedSourceBytes.private !== 0))) {
    throw new Error("provider egress sensitivity-byte evidence violates the destination boundary");
  }
}

function validateProviderObservationWindow(evidence) {
  const started = evidence.observationStartedMonotonicMs;
  const candidate = evidence.candidateStartedMonotonicMs;
  const terminated = evidence.processTreeTerminatedMonotonicMs;
  const finished = evidence.observationFinishedMonotonicMs;
  if (!(started < candidate && candidate <= terminated && terminated < finished)) {
    throw new Error("provider egress observation did not cover the candidate process tree");
  }
  return { candidate, terminated };
}

function validateProviderAuthorization(
  evidence, result, expected, networkTrustEvidence, bounds,
) {
  const { scenario, provider, committedEvents } = expected;
  const authorization = evidence.authorization;
  if (!authorization ||
      authorization.committedEventCount !== result.counts.committed) {
    throw new Error("provider authorization event count does not match committed events");
  }
  if (authorization.committedEventSetFingerprint !==
      providerEgressCommittedEventSetFingerprint(
        committedEvents, scenario.sourceRepositoryScope,
      )) {
    throw new Error("provider authorization event set does not match committed events");
  }
  if (!(bounds.candidate < authorization.observedAtMonotonicMs &&
      authorization.observedAtMonotonicMs <
        evidence.firstProviderRequestStartedMonotonicMs &&
      evidence.firstProviderRequestStartedMonotonicMs <=
        evidence.lastProviderRequestFinishedMonotonicMs &&
      evidence.lastProviderRequestFinishedMonotonicMs <= bounds.terminated)) {
    throw new Error("provider request is not strictly after runner-owned authorization");
  }
  const endpoint = new URL(provider.endpointUrl);
  const hostname = endpoint.hostname;
  const port = Number(endpoint.port || 443);
  const preflights = networkTrustEvidence.tlsPreflightReceipts.filter((receipt) =>
    receipt.hostname === hostname && receipt.port === port);
  const verifiedTls = provider.tlsPolicy === "system" && preflights.length === 2 &&
    Math.max(...preflights.map((receipt) => receipt.endMonotonicMs)) <
      authorization.observedAtMonotonicMs;
  const unverifiedLocalHttp = provider.executionLocation === "local" &&
    provider.tlsPolicy === "not_applicable" && preflights.length === 0;
  if (!verifiedTls && !unverifiedLocalHttp) {
    throw new Error("provider request is not after the exact verified TLS preflights");
  }
}

export function validateProviderEgressEvidence(
  evidence, result, fixture, networkTrustEvidence, attempt = {},
) {
  const committedEventIds = evidence.authorization?.committedEventIds ?? [];
  const expected = expectedProviderEgressObservation(
    result, fixture, committedEventIds, attempt,
  );
  validateProviderEgressClaims(evidence, expected);
  const bounds = validateProviderObservationWindow(evidence);
  if (expected.providerRequestCount === 0) {
    if (evidence.authorization !== null ||
        evidence.firstProviderRequestStartedMonotonicMs !== null ||
        evidence.lastProviderRequestFinishedMonotonicMs !== null ||
        evidence.credentialBytesSent !== 0 || evidence.payloadBytesSent !== 0 ||
        Object.values(evidence.sourcePayloadBytesBySensitivity).some((value) => value !== 0)) {
      throw new Error("zero-egress scenario contains an authorization or provider write");
    }
    return;
  }
  validateProviderAuthorization(evidence, result, expected, networkTrustEvidence, bounds);
}

function observedScenarioEvents(result, scenario) {
  const milestones = new Set(result.milestones.map((item) => item.name));
  const captureCompleted = milestones.has("source_events_captured") ||
    milestones.has("source_events_accepted");
  return result.drain.timedOut && !captureCompleted
    ? scenario.events.slice(0, result.counts.captured)
    : scenario.events;
}

function assertNoEgressBeforeProviderAttempt(
  result, scenario, fixture, observedRequests, observedPayloads,
) {
  const completionMilestone = scenario.providerTransmissionOracle.completionMilestone;
  const profile = fixture.lifecycleProfiles[scenario.lifecycleProfileId];
  const promptAttemptMilestone = "source_flush_requested_by_target_prompt";
  const attemptMilestone = profile.includes(promptAttemptMilestone)
    ? promptAttemptMilestone : completionMilestone;
  const attemptObserved = attemptMilestone !== null &&
    result.milestones.some((milestone) => milestone.name === attemptMilestone);
  const egressObserved = observedRequests !== 0 || observedPayloads !== 0 ||
    result.securityEvidence.credentialBytesSent !== 0 ||
    result.securityEvidence.payloadBytesSent !== 0;
  if (egressObserved && result.counts.committed === 0) {
    throw new Error("provider egress exists without committed events");
  }
  if (result.drain.timedOut && !attemptObserved && egressObserved) {
    throw new Error("provider egress exists before the observed attempt boundary");
  }
}

function evaluateDenominators(result, scenario, activeSummaryProvider, expectedDuplicateDeliveries, oracle) {
  const observedEvents = observedScenarioEvents(result, scenario);
  const remoteEvents = activeSummaryProvider.executionLocation === "remote" ? observedEvents : [];
  const eligibleCount = remoteEvents.filter((event) => event.sensitivity === "eligible").length;
  const milestones = new Set(result.milestones.map((item) => item.name));
  const extraMilestones = {
    consideredCrossScopeCandidateCount: "cross_scope_candidate_omitted",
    consideredDerivedCandidateCount: "local_only_derived_candidates_omitted",
    consideredActivationProposalCount: "provider_activation_proposed",
  };
  const duplicateDeliveries = result.drain.timedOut &&
    !milestones.has("stable_batch_replayed_second_time") ? 0 : expectedDuplicateDeliveries;
  return result.securityDenominators.agentOperationCount === 1 &&
    result.counts.captured === observedEvents.length &&
    result.securityDenominators.acceptedEventCount === result.counts.captured &&
    result.counts.duplicateDeliveries === duplicateDeliveries &&
    result.securityDenominators.duplicateDeliveryAttemptCount === duplicateDeliveries &&
    result.securityDenominators.consideredRemoteProviderEventCount === remoteEvents.length &&
    result.securityDenominators.consideredEligibleEventCount === eligibleCount &&
    result.securityDenominators.consideredRestrictedEventCount === remoteEvents.length - eligibleCount &&
    result.securityDenominators.consideredSecretEventCount ===
      observedEvents.filter((event) => event.sensitivity === "secret").length &&
    result.securityDenominators.consideredPrivateEventCount ===
      observedEvents.filter((event) => event.sensitivity === "private").length &&
    Object.entries(extraMilestones).every(
      ([name, milestone]) => !Object.hasOwn(oracle, name) ||
        result.securityDenominators[name] ===
          (!result.drain.timedOut || milestones.has(milestone) ? oracle[name] : 0),
    );
}

function evaluateProviderEvidence(result, scenario, fixture, activeSummaryProvider, exceptionalState) {
  const stub = scenario.summaryProviderStub;
  const requested = Object.hasOwn(stub, "summary") || Object.hasOwn(stub, "malformedResponse") ||
    Object.hasOwn(stub, "redirectResponse") || stub.memoryItems.length > 0;
  const observedEvents = observedScenarioEvents(result, scenario);
  const remoteExpected = activeSummaryProvider.executionLocation === "remote" && requested &&
    observedEvents.some((event) => event.sensitivity === "eligible") &&
    !Object.hasOwn(stub, "policyRejectedReason");
  const expectedRequests = remoteExpected ? (scenario.fault?.attemptsUntilExhausted ?? 1) : 0;
  const observedRequests = result.securityEvidence.remoteProviderRequestCount, observedPayloads = result.securityEvidence.remoteProviderPayloadCount;
  assertNoEgressBeforeProviderAttempt(
    result, scenario, fixture, observedRequests, observedPayloads,
  );
  const denominatorsPositive = result.securityDenominators.consideredRemoteProviderEventCount > 0 &&
    result.securityDenominators.consideredEligibleEventCount > 0;
  const completionMilestone = scenario.providerTransmissionOracle.completionMilestone;
  const providerCompletionObserved = completionMilestone !== null && result.milestones.some(
    (milestone) => milestone.name === completionMilestone,
  );
  const completedRouting = remoteExpected
    ? denominatorsPositive && observedRequests === expectedRequests && observedPayloads === expectedRequests
    : observedRequests === 0 && observedPayloads === 0;
  const timeoutRouting = remoteExpected
    ? denominatorsPositive && observedRequests <= expectedRequests && observedPayloads === observedRequests
      && (!providerCompletionObserved || observedRequests === expectedRequests)
    : observedRequests === 0 && observedPayloads === 0;
  const wire = scenario.providerTransmissionOracle;
  const exactWire = result.securityEvidence.credentialBytesSent === wire.credentialBytesSent &&
    result.securityEvidence.payloadBytesSent === wire.payloadBytesSent;
  const proportionalWire = observedRequests <= expectedRequests && observedPayloads === observedRequests &&
    (expectedRequests === 0
      ? result.securityEvidence.credentialBytesSent === 0 &&
        result.securityEvidence.payloadBytesSent === 0
      :
      result.securityEvidence.credentialBytesSent * expectedRequests ===
        wire.credentialBytesSent * observedRequests &&
      result.securityEvidence.payloadBytesSent * expectedRequests === wire.payloadBytesSent * observedRequests);
  const effectiveWire = result.drain.timedOut ? proportionalWire : exactWire;
  if (!exceptionalState && !effectiveWire) {
    throw new Error("provider transmission bytes do not match the pinned wire aggregate");
  }
  return {
    providerRoutingPass: result.drain.timedOut ? timeoutRouting : completedRouting,
    effectiveTransmissionPass: effectiveWire,
  };
}

function evaluateZeroToleranceEvidence(result, scenario, fixture, oracle) {
  const evidence = result.securityEvidence;
  const destination = fixture.effectiveConfiguration.destinationPolicyMap[scenario.targetDestinationClass];
  const incompatibleInjections = result.injectedItems.filter(
    (item) => !destination.eligibleSensitivities.includes(item.sensitivity),
  ).length;
  const crossScopeInjections = scenario.sourceRepositoryScope === scenario.targetRepositoryScope
    ? 0 : result.injectedItems.length;
  const deliveredPayload = result.finalRenderEvidence?.utf8Payload ?? "";
  const forbiddenSentinelObservations = Number((oracle.forbiddenSentinels ?? []).some(
    (sentinel) => deliveredPayload.includes(sentinel),
  ));
  return evidence.persistedSecretCount === 0 && evidence.injectedSecretCount === 0 &&
    evidence.restrictedPayloadBytesSent === 0 && evidence.redirectLocationRequestCount === 0 &&
    evidence.redirectLocationPayloadBytesSent === 0 && evidence.resentPayloadCount === 0 &&
    evidence.forbiddenSentinelObservationCount === forbiddenSentinelObservations &&
    forbiddenSentinelObservations === 0 &&
    incompatibleInjections === 0 && evidence.remoteInjectionCount === incompatibleInjections &&
    crossScopeInjections === 0 &&
    result.safety.incompatibleScopeInjectionCount === crossScopeInjections &&
    ORACLE_EVIDENCE_FIELDS.every((name) =>
      !Object.hasOwn(oracle, name) || oracle[name] !== 0 || evidence[name] === 0
    );
}

export function evaluateSecurityEvidence({
  result, scenario, fixture, exceptionalState, expectedDuplicateDeliveries,
}) {
  const oracle = scenario.securityOracle ?? {};
  const activeSummaryProvider = scenario.derivationManifestId
    ? fixture.localDerivationManifest.summaryProvider
    : fixture.effectiveConfiguration.summaryProvider;
  const denominatorsPass = evaluateDenominators(
    result, scenario, activeSummaryProvider, expectedDuplicateDeliveries, oracle,
  );
  const { providerRoutingPass, effectiveTransmissionPass } = evaluateProviderEvidence(
    result, scenario, fixture, activeSummaryProvider, exceptionalState,
  );
  const zeroToleranceSecurityEvidencePass = evaluateZeroToleranceEvidence(
    result, scenario, fixture, oracle,
  );
  const exactOraclePass = result.drain.timedOut || ORACLE_EVIDENCE_FIELDS.every(
    (name) => !Object.hasOwn(oracle, name) || result.securityEvidence[name] === oracle[name],
  );
  const securityEvidencePass = zeroToleranceSecurityEvidencePass && providerRoutingPass &&
    effectiveTransmissionPass && exactOraclePass &&
    (result.securityEvidence.remoteProviderRequestCount === 0 ||
      result.securityDenominators.consideredRemoteProviderEventCount > 0);
  const safetyCountersPass = isDeepStrictEqual(result.safety, scenario.expectedCounters) &&
    result.counts.lost === result.safety.acceptedEventLossCount;
  return { oracle, activeSummaryProvider, denominatorsPass, safetyCountersPass,
    securityEvidencePass };
}
