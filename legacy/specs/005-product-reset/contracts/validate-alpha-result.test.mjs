import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

import { canonicalizeJson } from "../../../harness/schema/jcs.ts";
import { buildRenderPayload, tokenizeRenderPayload } from "./alpha-result-render.mjs";
import {
  networkTrustEvidenceFingerprint,
  resourcePlateauEvidenceFingerprint,
  runnerEvidenceFingerprint,
  runnerResultObservationFingerprint,
  validateRunnerEvidence,
} from "./alpha-runner-evidence.mjs";
import { clearProviderEgressEvidence } from "./provider-egress-test-helper.mjs";
import "./validate-slice1-fixture.test.mjs";
import "./validate-alpha-runner-evidence.test.mjs";
import "./validate-alpha-result-render.test.mjs";
import "./validate-alpha-result-failure.test.mjs";
import "./validate-alpha-result-input.test.mjs";

const contractDir = dirname(fileURLToPath(import.meta.url)), repoRoot = join(contractDir, "../../..");
const validatorPath = join(contractDir, "validate-alpha-result.mjs");
const fixture = JSON.parse(readFileSync(join(contractDir,
  "../fixtures/slice1-bidirectional-en-v1.json"), "utf8"));
const success = JSON.parse(readFileSync(join(contractDir,
  "../fixtures/alpha-result-v1.example.json"), "utf8"));
const failure = JSON.parse(readFileSync(join(contractDir,
  "../fixtures/alpha-result-v1.failure-example.json"), "utf8"));
const suiteRegression = JSON.parse(readFileSync(join(contractDir,
  "../fixtures/alpha-result-v1.suite-regression.json"), "utf8"));
const suiteRegressionEvidence = JSON.parse(readFileSync(join(contractDir,
  "../fixtures/runner-evidence/alpha-runner-evidence-v1.suite-regression.json"), "utf8"));
const runnerEvidenceRoot = mkdtempSync(join(tmpdir(), "free-mem-alpha-runner-evidence-"));
let runnerEvidenceOrdinal = 0;
process.on("exit", () => rmSync(runnerEvidenceRoot, { recursive: true, force: true }));

function suiteProviderEgressEvidence(caseId) {
  const record = suiteRegressionEvidence.scenarios.find((item) => item.caseId === caseId);
  if (!record) throw new Error(`suite provider egress evidence is missing: ${caseId}`);
  return structuredClone(record.providerEgressEvidence);
}

function buildRunPreparations(result, latencyRuns) {
  return latencyRuns.map((run) => {
    const observations = run.captureTimings.flatMap(
      (timing) => [timing.startMonotonicMs, timing.endMonotonicMs],
    );
    for (const timing of [run.warmInjectionTiming, run.coldLexicalInjectionTiming]) {
      if (timing) observations.push(timing.startMonotonicMs, timing.endMonotonicMs);
    }
    const cold = run.resetMode === "fresh_isolated_data";
    const runStartedMonotonicMs = observations.length > 0
      ? Math.min(...observations) : run.runOrdinal * 100000;
    const runFinishedMonotonicMs = observations.length > 0
      ? Math.max(...observations) : runStartedMonotonicMs + 1;
    return {
      runOrdinal: run.runOrdinal,
      mode: run.resetMode,
      receiptId: `${result.scenarioId}:run-${run.runOrdinal}:preparation-receipt`,
      observedAtMonotonicMs: Math.max(0,
        runStartedMonotonicMs - fixture.samplingProtocol.processSampleIntervalMs),
      runStartedMonotonicMs,
      runFinishedMonotonicMs,
      dataDirInstanceId: cold
        ? `${result.scenarioId}:run-${run.runOrdinal}:data-root`
        : `${result.scenarioId}:warm-data-root`,
      processGenerationId: cold
        ? `${result.scenarioId}:run-${run.runOrdinal}:process-generation`
        : `${result.scenarioId}:warm-process-generation`,
      observedProductProcessCount: cold ? 0 : 1,
      observedDataDirEntryCount: cold ? 0 : 1,
      readyProcessObserved: !cold,
    };
  });
}

function bindRunnerOwnedEvidence(evidence) {
  evidence.networkTrustEvidence.runnerInvocationId = evidence.invocationId;
  for (const receipt of evidence.networkTrustEvidence.tlsPreflightReceipts) {
    receipt.runnerInvocationId = evidence.invocationId;
  }
  if (evidence.resourcePlateauEvidence !== null) {
    Object.assign(evidence.resourcePlateauEvidence, {
      candidateId: evidence.candidateId,
      artifactFingerprint: evidence.artifactFingerprint,
      environmentFingerprint: evidence.environmentFingerprint,
      runnerInvocationId: evidence.invocationId,
      processTreeRootId: `resource-plateau:${evidence.invocationId}`,
    });
  }
  const [record] = evidence.scenarios;
  if (record.providerEgressEvidence.kind === "observed") {
    record.providerEgressEvidence.runnerInvocationId = evidence.invocationId;
    record.providerEgressEvidence.processTreeRootId = record.processTreeRootId;
  }
  for (const wrapper of record.recoveryProviderEgressEvidence) {
    wrapper.evidence.runnerInvocationId = evidence.invocationId;
    wrapper.evidence.processTreeRootId = wrapper.processTreeRootId;
  }
  return evidence;
}

function buildRunnerEvidence(result) {
  const scenario = fixture.scenarios.find((item) => item.scenarioId === result.scenarioId);
  const exceptional = result.disposition.state === "unsupported" ||
    result.disposition.state === "not_run";
  const recoveryObserved = result.milestones.some((item) =>
    item.name === scenario.drainCondition.terminalMilestone);
  const latencyRuns = structuredClone(result.latencyEvidence.runs);
  const runPreparations = buildRunPreparations(result, latencyRuns);
  const evidence = {
    runnerEvidenceVersion: 1,
    fixtureId: result.fixtureId,
    fixtureFingerprint: result.fixtureFingerprint,
    candidateId: result.candidateId,
    environmentFingerprint: result.environmentFingerprint,
    artifactFingerprint: result.artifactFingerprint,
    runnerId: "fixture-pinned-reference-runner-v1",
    invocationId: `${result.candidateId}:fixture-invocation-v1`,
    networkTrustEvidence: structuredClone(suiteRegressionEvidence.networkTrustEvidence),
    resourcePlateauEvidence: exceptional
      ? null : structuredClone(suiteRegressionEvidence.resourcePlateauEvidence),
    scenarios: [{
      caseId: result.runnerEvidenceCaseId,
      scenarioId: result.scenarioId,
      resourceSampleMode: result.resourceSampleMode,
      resourceObserverId: "fixture-pinned-resource-observer-v1",
      processTreeRootId: `${result.scenarioId}:process-tree-root`,
      resourceDataRootId: `${result.scenarioId}:resource-data-root`,
      resultObservationFingerprint: runnerResultObservationFingerprint(result),
      providerEgressEvidence: suiteProviderEgressEvidence(result.runnerEvidenceCaseId),
      recoveryProviderEgressEvidence:
        !recoveryObserved
          ? []
          : structuredClone(suiteRegressionEvidence.scenarios.find(
              (item) => item.caseId === result.runnerEvidenceCaseId,
            ).recoveryProviderEgressEvidence),
      hostIdentityEvidence: structuredClone(result.hostIdentityEvidence),
      observedMilestones: structuredClone(result.milestones),
      processSamples: structuredClone(result.processSamples),
      latencyRuns,
      runPreparations,
    }],
  };
  return bindRunnerOwnedEvidence(evidence);
}

function attachRunnerEvidence(result, evidence) {
  result.resourcePlateauEvidenceFingerprint = evidence.resourcePlateauEvidence === null
    ? null : resourcePlateauEvidenceFingerprint(evidence.resourcePlateauEvidence);
  result.runnerEvidenceFingerprint = runnerEvidenceFingerprint(evidence);
  return result;
}

const successEvidence = buildRunnerEvidence(success);
const failureEvidence = buildRunnerEvidence(failure);
attachRunnerEvidence(success, successEvidence);
attachRunnerEvidence(failure, failureEvidence);

function writeRunnerEvidence(evidence) {
  const path = join(runnerEvidenceRoot, `runner-evidence-${runnerEvidenceOrdinal += 1}.json`);
  writeFileSync(path, JSON.stringify(evidence), { mode: 0o600 });
  return path;
}

function validate(result, evidence = null,
  expectedInvocationId = `${result.candidateId}:fixture-invocation-v1`) {
  if (evidence === null) {
    evidence = buildRunnerEvidence(result);
    attachRunnerEvidence(result, evidence);
  }
  const evidencePath = writeRunnerEvidence(evidence);
  return spawnSync(process.execPath,
    ["--experimental-strip-types", validatorPath,
      "--runner-evidence-root", runnerEvidenceRoot,
      "--runner-evidence", evidencePath,
      "--runner-invocation-id", expectedInvocationId, "--result", "-"], {
      cwd: repoRoot,
      input: JSON.stringify(result),
      encoding: "utf8",
    });
}

function assertAccepted(result, label, evidence, expectedInvocationId) {
  const run = validate(result, evidence, expectedInvocationId);
  assert.equal(run.status, 0, `${label}: ${run.stderr}${run.stdout}`);
}

function assertRejected(result, pattern, label, evidence, expectedInvocationId) {
  const run = validate(result, evidence, expectedInvocationId);
  assert.notEqual(run.status, 0, `${label}: unexpectedly accepted`);
  assert.match(`${run.stderr}${run.stdout}`, pattern, label);
}

function processSamplesThrough(template, endMs, terminalMs = endMs) {
  const start = template.processSamples[0];
  const steady = { ...template.processSamples[1], processCount: 2 };
  const samples = [];
  for (let monotonicMs = 0; monotonicMs < terminalMs; monotonicMs += 100) {
    samples.push({ ...(monotonicMs === 0 ? start : steady), monotonicMs });
  }
  if (endMs > terminalMs && samples.at(-1)?.monotonicMs !== terminalMs) {
    samples.push({ ...steady, monotonicMs: terminalMs });
  }
  samples.push({ ...template.processSamples.at(-1), monotonicMs: endMs });
  return samples;
}

function timedOutBeforeProviderTerminal() {
  const result = structuredClone(failure);
  const terminalIndex = result.milestones.findIndex(
    (item) => item.name === result.drain.terminalMilestone,
  );
  assert.notEqual(terminalIndex, -1, "failure fixture does not contain its terminal milestone");
  result.drain = { ...result.drain, status: "timed_out", timedOut: true };
  result.milestones = result.milestones.slice(0, terminalIndex);
  result.disposition = {
    state: "failed",
    reason: "drain_timed_out",
    successfulComparisonEligible: false,
  };
  result.retryEvidence = null;
  result.failureMetadata = null;
  result.operationalStatus = null;
  result.outputLimitAtomicityEvidence = null;
  result.processSamples = processSamplesThrough(result, 30000);
  result.resource.maxSteadyProductProcessCount = 2;
  return result;
}

function completedAtBoundary(terminalMs) {
  const result = structuredClone(success);
  const setTime = (name, monotonicMs) => {
    result.milestones.find((item) => item.name === name).monotonicMs = monotonicMs;
  };
  setTime("target_injection_acknowledged", terminalMs);
  setTime("target_model_request_dispatched", terminalMs + 1);
  setTime("scenario_terminal", terminalMs + 2);
  setTime("post_teardown_grace_elapsed", terminalMs + 3);
  result.processSamples = processSamplesThrough(result, terminalMs + 3, terminalMs + 2);
  return result;
}

function renderEvidence(result, scenario, items, packId) {
  const payload = canonicalizeJson(buildRenderPayload(result, scenario, fixture, items, packId));
  return {
    evidence: {
      rendererId: "alpha-jcs-renderer-v1",
      utf8Payload: payload,
      tokenizerId: "deterministic-fixture-tokenizer-v1",
      tokenizerRevision: "1",
      tokenIds: tokenizeRenderPayload(payload),
    },
    bytes: Buffer.byteLength(payload, "utf8"),
  };
}

function unsupportedPackFailure() {
  // Keep this valid under the former contract so the test proves removal, not only schema rejection.
  const result = structuredClone(success);
  const scenario = fixture.scenarios.find((item) => item.scenarioId === result.scenarioId);
  result.packDegradations = ["x".repeat(17000)];
  result.packCompilationFailure = "injection_pack_limit_exceeded";
  result.disposition = {
    state: "failed",
    reason: "injection_pack_limit_exceeded",
    successfulComparisonEligible: false,
  };
  result.injectedItems = [];
  result.omittedItems = [];
  for (const name of ["inputCandidates", "tracedCandidates", "deadlineUnprocessed",
    "admittedCandidates", "selectedItems"]) result.counts[name] = 0;
  result.quality = {
    expectedInjectedItemCount: scenario.expectedInjectedItems.length,
    matchedInjectedItemCount: 0,
    expectedOmissionCount: scenario.expectedOmissions.length,
    matchedOmissionCount: 0,
    forbiddenFactCount: 0,
  };
  result.attemptedItems = [];
  result.packId = null;
  result.finalRenderEvidence = null;
  result.renderedBytes = 0;
  result.injectedTokens = 0;
  const attempted = renderEvidence(result, scenario, [], null);
  result.attemptedRenderEvidence = attempted.evidence;
  result.attemptedRenderedBytes = attempted.bytes;
  result.attemptedInjectedTokens = result.attemptedRenderEvidence.tokenIds.length;
  return result;
}

function oversizedFinalPack(packIdSuffix) {
  const result = structuredClone(success);
  const scenario = fixture.scenarios.find((item) => item.scenarioId === result.scenarioId);
  result.packId = `${result.packId}:${packIdSuffix}`;
  const final = renderEvidence(result, scenario, result.injectedItems, result.packId);
  result.finalRenderEvidence = final.evidence;
  result.attemptedRenderEvidence = "same_as_final";
  result.renderedBytes = final.bytes;
  result.attemptedRenderedBytes = final.bytes;
  result.injectedTokens = final.evidence.tokenIds.length;
  result.attemptedInjectedTokens = final.evidence.tokenIds.length;
  return result;
}

function unsupportedResult() {
  const result = structuredClone(failure);
  result.disposition = {
    state: "unsupported",
    reason: "capability_unsupported",
    successfulComparisonEligible: false,
  };
  result.drain.timedOut = false;
  result.milestones = [];
  result.injectionBeforeModel = null;
  result.hostIdentityEvidence = null;
  result.identityConflictEvidence = null;
  result.retryEvidence = null;
  result.failureMetadata = null;
  result.outputLimitAtomicityEvidence = null;
  result.operationalStatus = null;
  for (const object of [result.counts, result.safety, result.securityDenominators,
    result.securityEvidence, result.resource]) {
    for (const name of Object.keys(object)) object[name] = 0;
  }
  result.processSamples = [0, 100].map((monotonicMs) => ({
    monotonicMs, processCount: 0, rssMiB: 0, queueDepth: 0, storageBytes: 0,
  }));
  result.injectedItems = [];
  result.omittedItems = [];
  result.packId = null;
  result.packDegradations = [];
  result.attemptedItems = [];
  result.attemptedRenderEvidence = null;
  result.finalRenderEvidence = null;
  result.attemptedRenderedBytes = 0;
  result.renderedBytes = 0;
  result.attemptedInjectedTokens = 0;
  result.injectedTokens = 0;
  result.selectionTimingEvidence = null;
  result.selectionElapsedMs = 0;
  result.latencyEvidence = {
    captureEventIds: [], runs: [],
    aggregates: { captureP95Ms: null, warmInjectionP95Ms: null,
      shortColdLexicalInjectionMs: null },
  };
  result.providerCostUnits = null;
  result.resourcePlateauEvidenceFingerprint = null;
  return result;
}

assertAccepted(success, "positive example");
assertAccepted(failure, "failure example");

const missingDeterministicStubCost = structuredClone(success);
missingDeterministicStubCost.providerCostUnits = null;
assertRejected(missingDeterministicStubCost, /deterministic provider stub/,
  "runner-owned deterministic stub cost was inferred from provider cost class");

const unsupported = unsupportedResult();
const unsupportedEvidence = buildRunnerEvidence(unsupported);
clearProviderEgressEvidence(unsupportedEvidence, unsupported.runnerEvidenceCaseId);
attachRunnerEvidence(unsupported, unsupportedEvidence);
assertAccepted(unsupported, "unsupported result", unsupportedEvidence);

const unsupportedWithPlateau = structuredClone(unsupported);
const unsupportedWithPlateauEvidence = buildRunnerEvidence(unsupportedWithPlateau);
unsupportedWithPlateauEvidence.resourcePlateauEvidence =
  structuredClone(suiteRegressionEvidence.resourcePlateauEvidence);
bindRunnerOwnedEvidence(unsupportedWithPlateauEvidence);
attachRunnerEvidence(unsupportedWithPlateau, unsupportedWithPlateauEvidence);
assert.throws(() => validateRunnerEvidence(
  unsupportedWithPlateauEvidence, unsupportedWithPlateau, fixture,
  unsupportedWithPlateauEvidence.invocationId,
), /unsupported\/not-run runner evidence contains plateau workload/,
  "shared runner validator accepted plateau work for an unsupported result");
assertRejected(unsupportedWithPlateau,
  /expected type null/,
  "unsupported result carried executed plateau evidence", unsupportedWithPlateauEvidence);

const zeroReportedObservations = structuredClone(success);
for (const run of zeroReportedObservations.latencyEvidence.runs) {
  for (const timing of run.captureTimings) {
    timing.startMonotonicMs = 0;
    timing.endMonotonicMs = 0;
  }
  for (const timing of [run.warmInjectionTiming, run.coldLexicalInjectionTiming]) {
    if (timing) {
      timing.startMonotonicMs = 0;
      timing.endMonotonicMs = 0;
    }
  }
}
for (const name of Object.keys(zeroReportedObservations.latencyEvidence.aggregates)) {
  if (zeroReportedObservations.latencyEvidence.aggregates[name] !== null) {
    zeroReportedObservations.latencyEvidence.aggregates[name] = 0;
  }
}
for (const sample of zeroReportedObservations.processSamples) {
  sample.processCount = 0;
  sample.rssMiB = 0;
  sample.queueDepth = 0;
  sample.storageBytes = 0;
}
for (const name of Object.keys(zeroReportedObservations.resource)) {
  zeroReportedObservations.resource[name] = 0;
}
assertRejected(zeroReportedObservations, /runner evidence/,
  "candidate-authored zero latency and resource evidence", successEvidence);

const timeout = timedOutBeforeProviderTerminal();
const timeoutEvidence = buildRunnerEvidence(timeout);
attachRunnerEvidence(timeout, timeoutEvidence);
assertAccepted(timeout, "timeout before provider terminal", timeoutEvidence);
for (const field of ["retryEvidence", "failureMetadata", "operationalStatus"]) {
  const isolated = structuredClone(timeout);
  isolated[field] = failure[field];
  const isolatedEvidence = buildRunnerEvidence(isolated);
  attachRunnerEvidence(isolated, isolatedEvidence);
  assertRejected(isolated, /provider failure evidence does not match observed lifecycle/,
    `timeout with isolated ${field}`, isolatedEvidence);
}
const spoolConflictEvidence = suiteRegression.positiveResults.find(
  (result) => result.scenarioId === "runtime-unavailable-spool-recovery",
).identityConflictEvidence;
const timeoutWithConflictEvidence = structuredClone(timeout);
timeoutWithConflictEvidence.identityConflictEvidence = structuredClone(spoolConflictEvidence);
const timeoutWithConflictRunnerEvidence = buildRunnerEvidence(timeoutWithConflictEvidence);
attachRunnerEvidence(timeoutWithConflictEvidence, timeoutWithConflictRunnerEvidence);
assertRejected(timeoutWithConflictEvidence,
  /identity conflict evidence does not match observed lifecycle/,
  "timeout claimed an unobserved identity conflict", timeoutWithConflictRunnerEvidence);

const beforeBoundary = completedAtBoundary(29999);
const beforeBoundaryEvidence = buildRunnerEvidence(beforeBoundary);
attachRunnerEvidence(beforeBoundary, beforeBoundaryEvidence);
assertAccepted(beforeBoundary, "completion before timeout boundary", beforeBoundaryEvidence);
const atBoundary = completedAtBoundary(30000);
const atBoundaryEvidence = buildRunnerEvidence(atBoundary);
attachRunnerEvidence(atBoundary, atBoundaryEvidence);
assertRejected(atBoundary, /completed drain reached or exceeded the pinned timeout/,
  "completion at timeout boundary", atBoundaryEvidence);

const resourceThresholdFailure = structuredClone(success);
resourceThresholdFailure.disposition = {
  state: "failed",
  reason: "resource_threshold_exceeded",
  successfulComparisonEligible: false,
};
const failedPlateau = structuredClone(suiteRegressionEvidence.resourcePlateauEvidence);
failedPlateau.windows[3].rssMiB = failedPlateau.windows[2].rssMiB + 33;
resourceThresholdFailure.resourcePlateauEvidenceFingerprint =
  resourcePlateauEvidenceFingerprint(failedPlateau);
const resourceThresholdEvidence = buildRunnerEvidence(resourceThresholdFailure);
resourceThresholdEvidence.resourcePlateauEvidence = failedPlateau;
bindRunnerOwnedEvidence(resourceThresholdEvidence);
resourceThresholdFailure.resourcePlateauEvidenceFingerprint =
  resourcePlateauEvidenceFingerprint(resourceThresholdEvidence.resourcePlateauEvidence);
resourceThresholdEvidence.scenarios[0].resultObservationFingerprint =
  runnerResultObservationFingerprint(resourceThresholdFailure);
attachRunnerEvidence(resourceThresholdFailure, resourceThresholdEvidence);
assertAccepted(resourceThresholdFailure,
  "resource threshold miss remains an inspectable failed result", resourceThresholdEvidence);

const inconsistentResourceThreshold = structuredClone(resourceThresholdFailure);
inconsistentResourceThreshold.resource.maxSteadyProductProcessCount += 1;
const inconsistentResourceEvidence = buildRunnerEvidence(inconsistentResourceThreshold);
inconsistentResourceEvidence.resourcePlateauEvidence = structuredClone(failedPlateau);
bindRunnerOwnedEvidence(inconsistentResourceEvidence);
inconsistentResourceThreshold.resourcePlateauEvidenceFingerprint =
  resourcePlateauEvidenceFingerprint(inconsistentResourceEvidence.resourcePlateauEvidence);
inconsistentResourceEvidence.scenarios[0].resultObservationFingerprint =
  runnerResultObservationFingerprint(inconsistentResourceThreshold);
attachRunnerEvidence(inconsistentResourceThreshold, inconsistentResourceEvidence);
assertRejected(inconsistentResourceThreshold, /resource aggregates/,
  "resource threshold miss accepted inconsistent per-scenario aggregates",
  inconsistentResourceEvidence);

assertRejected(unsupportedPackFailure(), /unknown property/,
  "Slice 1 explicit pack-compilation failure");
const byteOversized = oversizedFinalPack("x".repeat(17000));
assert.ok(byteOversized.renderedBytes >
  fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.maxRenderedBytes);
assert.ok(byteOversized.injectedTokens <=
  fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.maxInjectedTokens);
assertRejected(byteOversized, /oversized InjectionPack was recorded as final output/,
  "byte-oversized final pack");
const tokenOversized = oversizedFinalPack("x ".repeat(801).trim());
assert.ok(tokenOversized.renderedBytes <=
  fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.maxRenderedBytes);
assert.ok(tokenOversized.injectedTokens >
  fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.maxInjectedTokens);
assertRejected(tokenOversized, /oversized InjectionPack was recorded as final output/,
  "token-oversized final pack");

for (const [field, pattern] of [
  ["receiptId", /runner preparation receipts are reused/],
  ["dataDirInstanceId", /cold preparation identities are reused/],
  ["processGenerationId", /cold preparation identities are reused/],
]) {
  const reusedColdPreparation = structuredClone(successEvidence);
  reusedColdPreparation.scenarios[0].runPreparations[1][field] =
    reusedColdPreparation.scenarios[0].runPreparations[0][field];
  const reusedColdResult = attachRunnerEvidence(structuredClone(success), reusedColdPreparation);
  assertRejected(reusedColdResult, pattern, `cold run reused ${field}`, reusedColdPreparation);
}

for (const [field, target] of [
  ["processTreeRootId", (evidence) => evidence.scenarios[0]],
  ["resourceDataRootId", (evidence) => evidence.scenarios[0]],
  ["receiptId", (evidence) => evidence.scenarios[0].runPreparations[0]],
  ["dataDirInstanceId", (evidence) => evidence.scenarios[0].runPreparations[0]],
  ["processGenerationId", (evidence) => evidence.scenarios[0].runPreparations[0]],
]) {
  const absolutePathIdentity = structuredClone(successEvidence);
  target(absolutePathIdentity)[field] = "/tmp/private-runner-state";
  const absolutePathResult = attachRunnerEvidence(
    structuredClone(success), absolutePathIdentity,
  );
  assertRejected(absolutePathResult, /string does not match pattern/,
    `runner evidence exposed an absolute ${field}`, absolutePathIdentity);
}

const splitWarmGeneration = structuredClone(failureEvidence);
splitWarmGeneration.scenarios[0].runPreparations[1].processGenerationId =
  `${failure.scenarioId}:unexpected-process-generation`;
const splitWarmResult = attachRunnerEvidence(structuredClone(failure), splitWarmGeneration);
assertRejected(splitWarmResult, /warm runner preparation did not prove retained ready state/,
  "warm run changed process generation", splitWarmGeneration);

const staleColdPreparation = structuredClone(successEvidence);
for (const preparation of staleColdPreparation.scenarios[0].runPreparations) {
  preparation.observedAtMonotonicMs = 0;
}
const staleColdResult = attachRunnerEvidence(structuredClone(success), staleColdPreparation);
assertRejected(staleColdResult, /runner preparation evidence does not match latency runs/,
  "cold reset receipts were not observed between runs", staleColdPreparation);

const simultaneousColdPreparation = structuredClone(successEvidence);
for (const preparation of simultaneousColdPreparation.scenarios[0].runPreparations) {
  preparation.observedAtMonotonicMs = preparation.runStartedMonotonicMs;
}
const simultaneousColdResult = attachRunnerEvidence(
  structuredClone(success), simultaneousColdPreparation,
);
assertRejected(simultaneousColdResult, /runner preparation evidence does not match latency runs/,
  "cold reset receipt was simultaneous with run start", simultaneousColdPreparation);

const emptyObservationPreparation = structuredClone(failureEvidence);
for (const run of emptyObservationPreparation.scenarios[0].latencyRuns) {
  run.captureTimings = [];
  run.warmInjectionTiming = null;
  run.coldLexicalInjectionTiming = null;
}
for (const preparation of emptyObservationPreparation.scenarios[0].runPreparations) {
  preparation.observedAtMonotonicMs = 0;
}
const emptyObservationResult = structuredClone(failure);
emptyObservationResult.latencyEvidence.runs = structuredClone(
  emptyObservationPreparation.scenarios[0].latencyRuns,
);
emptyObservationPreparation.scenarios[0].resultObservationFingerprint =
  runnerResultObservationFingerprint(emptyObservationResult);
attachRunnerEvidence(emptyObservationResult, emptyObservationPreparation);
assert.throws(() => validateRunnerEvidence(emptyObservationPreparation, emptyObservationResult,
  fixture, emptyObservationPreparation.invocationId),
  /runner preparation evidence does not match latency runs/);

const mismatchedRunnerIdentity = structuredClone(successEvidence);
mismatchedRunnerIdentity.candidateId = "another-candidate";
bindRunnerOwnedEvidence(mismatchedRunnerIdentity);
const mismatchedRunnerResult = structuredClone(success);
mismatchedRunnerResult.resourcePlateauEvidenceFingerprint =
  resourcePlateauEvidenceFingerprint(mismatchedRunnerIdentity.resourcePlateauEvidence);
mismatchedRunnerIdentity.scenarios[0].resultObservationFingerprint =
  runnerResultObservationFingerprint(mismatchedRunnerResult);
attachRunnerEvidence(mismatchedRunnerResult, mismatchedRunnerIdentity);
assertRejected(mismatchedRunnerResult, /runner evidence identity does not match the result/,
  "runner evidence candidate mismatch", mismatchedRunnerIdentity);

const replayedInvocation = structuredClone(successEvidence);
replayedInvocation.invocationId = "prior-runner-invocation";
bindRunnerOwnedEvidence(replayedInvocation);
const replayedInvocationResult = structuredClone(success);
replayedInvocationResult.networkTrustEvidenceFingerprint =
  networkTrustEvidenceFingerprint(replayedInvocation.networkTrustEvidence);
replayedInvocationResult.resourcePlateauEvidenceFingerprint =
  resourcePlateauEvidenceFingerprint(replayedInvocation.resourcePlateauEvidence);
replayedInvocation.scenarios[0].resultObservationFingerprint =
  runnerResultObservationFingerprint(replayedInvocationResult);
attachRunnerEvidence(replayedInvocationResult, replayedInvocation);
assertRejected(replayedInvocationResult, /runner evidence identity does not match the result/,
  "runner evidence invocation replay", replayedInvocation,
  successEvidence.invocationId);

const mismatchedHostIdentity = structuredClone(successEvidence);
mismatchedHostIdentity.scenarios[0].hostIdentityEvidence.effectiveIdentity.sessionId =
  "host-observed-other-session";
const mismatchedHostIdentityResult = attachRunnerEvidence(
  structuredClone(success), mismatchedHostIdentity,
);
assertRejected(mismatchedHostIdentityResult, /result observations do not match runner evidence/,
  "candidate copied host identity projection", mismatchedHostIdentity);

const unboundResultObservation = structuredClone(success);
unboundResultObservation.securityEvidence.payloadBytesSent += 1;
assert.throws(() => validateRunnerEvidence(successEvidence, unboundResultObservation,
  fixture, successEvidence.invocationId), /result observation fingerprint/);

const suiteScenarioIds = fixture.scenarios.map((item) => item.scenarioId).sort();
const suiteCaseIds = [...suiteScenarioIds, fixture.beforeModelNegativeFixture.caseId].sort();
const suiteRunnerEvidence = structuredClone(successEvidence);
suiteRunnerEvidence.scenarios = suiteCaseIds.map((caseId) => {
  const scenarioId = caseId === fixture.beforeModelNegativeFixture.caseId
    ? fixture.beforeModelNegativeFixture.baseScenarioId : caseId;
  const record = { ...structuredClone(successEvidence.scenarios[0]), caseId, scenarioId };
  record.processTreeRootId = `${caseId}:${record.processTreeRootId}`;
  record.providerEgressEvidence.receiptId =
    `${caseId}:${record.providerEgressEvidence.receiptId}`;
  record.providerEgressEvidence.observationCaseId = caseId;
  record.providerEgressEvidence.processTreeRootId = record.processTreeRootId;
  for (const preparation of record.runPreparations) {
    preparation.receiptId = `${caseId}:${preparation.receiptId}`;
    preparation.dataDirInstanceId = `${caseId}:${preparation.dataDirInstanceId}`;
    preparation.processGenerationId = `${caseId}:${preparation.processGenerationId}`;
  }
  return record;
});
const syntheticBaseEgress = suiteRunnerEvidence.scenarios.find((item) =>
  item.caseId === fixture.beforeModelNegativeFixture.baseScenarioId).providerEgressEvidence;
suiteRunnerEvidence.scenarios.find((item) =>
  item.caseId === fixture.beforeModelNegativeFixture.caseId).providerEgressEvidence = {
  kind: "projection",
  sourceCaseId: fixture.beforeModelNegativeFixture.baseScenarioId,
  sourceReceiptId: syntheticBaseEgress.receiptId,
};
const suiteRunnerResult = attachRunnerEvidence(structuredClone(success), suiteRunnerEvidence);
validateRunnerEvidence(suiteRunnerEvidence, suiteRunnerResult,
  fixture, suiteRunnerEvidence.invocationId, suiteCaseIds);
const incompleteSuiteEvidence = structuredClone(suiteRunnerEvidence);
incompleteSuiteEvidence.scenarios.pop();
const incompleteSuiteResult = attachRunnerEvidence(
  structuredClone(success), incompleteSuiteEvidence,
);
assert.throws(() => validateRunnerEvidence(incompleteSuiteEvidence, incompleteSuiteResult,
  fixture, incompleteSuiteEvidence.invocationId, suiteCaseIds),
  /runner evidence scenarios are duplicated, unsorted, or incomplete/);
const duplicateSuitePreparations = structuredClone(suiteRunnerEvidence);
duplicateSuitePreparations.scenarios[1].runPreparations = structuredClone(
  duplicateSuitePreparations.scenarios[0].runPreparations,
);
const duplicateSuiteResult = attachRunnerEvidence(
  structuredClone(success), duplicateSuitePreparations,
);
assert.throws(() => validateRunnerEvidence(duplicateSuitePreparations, duplicateSuiteResult,
  fixture, duplicateSuitePreparations.invocationId, suiteCaseIds),
  /runner preparation receipts are reused across the evidence bundle/);

const crossModePreparationReuse = structuredClone(suiteRegressionEvidence);
const coldRecord = crossModePreparationReuse.scenarios.find(
  (record) => record.resourceSampleMode === "cold",
);
const warmRecord = crossModePreparationReuse.scenarios.find(
  (record) => record.resourceSampleMode === "warm",
);
coldRecord.runPreparations[0].dataDirInstanceId = warmRecord.runPreparations[0].dataDirInstanceId;
const crossModeResult = structuredClone([...suiteRegression.positiveResults, suiteRegression.negativeResult].find((result) => result.runnerEvidenceCaseId === coldRecord.caseId));
attachRunnerEvidence(crossModeResult, crossModePreparationReuse);
assert.throws(() => validateRunnerEvidence(crossModePreparationReuse, crossModeResult,
  fixture, crossModePreparationReuse.invocationId, suiteCaseIds),
  /cold preparation identities are reused across the evidence bundle/);

console.log("Alpha result regression checks passed.");
