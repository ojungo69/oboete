import { createHash } from "node:crypto";
import { realpathSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { isDeepStrictEqual } from "node:util";

import { canonicalizeJson } from "../../../harness/schema/jcs.ts";
import { isWithin, readBoundedIJsonFile } from "./alpha-result-input.mjs";
import { validateResourcePlateauEvidence } from "./alpha-result-resource.mjs";
import { expectedRetryEvidence } from "./alpha-result-retry.mjs";
import {
  validateNetworkTrustEvidence,
  validateProviderEgressEvidence,
} from "./alpha-result-security.mjs";

export const MAX_RUNNER_EVIDENCE_BYTES = 1024 * 1024;

const fingerprint = (domain, value) => `sha256:${createHash("sha256")
  .update(domain).update(canonicalizeJson(value)).digest("hex")}`;

export function networkTrustEvidenceFingerprint(evidence) {
  return fingerprint("free-mem:alpha-network-trust-evidence:v1\0", evidence);
}

export function resourcePlateauEvidenceFingerprint(evidence) {
  return fingerprint("free-mem:alpha-resource-plateau-evidence:v1\0", evidence);
}

export function runnerEvidenceFingerprint(evidence) {
  return fingerprint("free-mem:alpha-runner-evidence:v1\0", evidence);
}

export function runnerResultObservationFingerprint(result) {
  const { runnerEvidenceFingerprint: _runnerEvidenceFingerprint, ...observation } = result;
  return fingerprint("free-mem:alpha-runner-result-observation:v1\0", observation);
}

export function readRunnerEvidenceFile(path, evidenceRoot, artifactRoot) {
  const root = realpathSync(resolve(evidenceRoot));
  const artifact = realpathSync(resolve(artifactRoot));
  const rootStat = statSync(root);
  if (!rootStat.isDirectory() || (rootStat.mode & 0o022) !== 0 ||
      (typeof process.getuid === "function" && rootStat.uid !== process.getuid())) {
    throw new Error("runner evidence root is not runner-owned and immutable");
  }
  if (isWithin(artifact, root) || isWithin(root, artifact)) {
    throw new Error("runner evidence root overlaps the candidate artifact root");
  }
  try {
    return readBoundedIJsonFile(path, MAX_RUNNER_EVIDENCE_BYTES, root);
  } catch (error) {
    throw new Error(`runner evidence ${error instanceof Error ? error.message : "read failed"}`);
  }
}

const observationTimes = (run) => [
  ...run.captureTimings.flatMap((timing) => [timing.startMonotonicMs, timing.endMonotonicMs]),
  ...[run.warmInjectionTiming, run.coldLexicalInjectionTiming]
    .filter(Boolean)
    .flatMap((timing) => [timing.startMonotonicMs, timing.endMonotonicMs]),
];

function validateRunPreparations(
  record, result, exceptionalState, maxPreparationGapMs, maxProductProcessCount,
) {
  const runs = record.latencyRuns;
  const preparations = record.runPreparations;
  if (exceptionalState) {
    if (runs.length !== 0 || preparations.length !== 0) {
      throw new Error("exceptional runner evidence contains measured runs");
    }
    return;
  }
  if (preparations.length !== runs.length || !preparations.every((item, index) => {
    const times = observationTimes(runs[index]);
    return item.runOrdinal === runs[index].runOrdinal && item.mode === runs[index].resetMode &&
      item.runStartedMonotonicMs <= item.runFinishedMonotonicMs &&
      item.observedAtMonotonicMs < item.runStartedMonotonicMs &&
      item.runStartedMonotonicMs - item.observedAtMonotonicMs <= maxPreparationGapMs &&
      (result.resourceSampleMode !== "cold" || (times.length > 0 &&
        Math.min(...times) - item.runStartedMonotonicMs <= maxPreparationGapMs)) &&
      times.every((time) =>
        time >= item.runStartedMonotonicMs && time <= item.runFinishedMonotonicMs) &&
      (index === 0 || (item.observedAtMonotonicMs > preparations[index - 1].runFinishedMonotonicMs &&
        item.runStartedMonotonicMs > preparations[index - 1].runFinishedMonotonicMs));
  })) {
    throw new Error("runner preparation evidence does not match latency runs");
  }
  const dataRoots = new Set(preparations.map((item) => item.dataDirInstanceId));
  const processGenerations = new Set(preparations.map((item) => item.processGenerationId));
  if (result.resourceSampleMode === "cold") {
    if (!preparations.every((item) => item.observedProductProcessCount === 0 &&
          item.observedDataDirEntryCount === 0 && !item.readyProcessObserved)) {
      throw new Error("cold runner preparation did not prove an isolated reset");
    }
  } else if (dataRoots.size !== 1 || processGenerations.size !== 1 ||
      !preparations.every((item) => item.observedProductProcessCount > 0 &&
        item.observedProductProcessCount <= maxProductProcessCount &&
        item.readyProcessObserved)) {
    throw new Error("warm runner preparation did not prove retained ready state");
  }
}

function validateBundlePreparationIdentities(evidence) {
  const preparations = evidence.scenarios.flatMap((record) => record.runPreparations);
  if (new Set(preparations.map((item) => item.receiptId)).size !== preparations.length) {
    throw new Error("runner preparation receipts are reused across the evidence bundle");
  }
  const cold = evidence.scenarios.filter((record) => record.resourceSampleMode === "cold")
    .flatMap((record) => record.runPreparations);
  for (const name of ["dataDirInstanceId", "processGenerationId"]) {
    const occurrences = new Map();
    for (const item of preparations) occurrences.set(item[name], (occurrences.get(item[name]) ?? 0) + 1);
    if (cold.some((item) => occurrences.get(item[name]) !== 1)) {
      throw new Error("cold preparation identities are reused across the evidence bundle");
    }
  }
}

function resolveProviderEgressEvidence(evidence, record) {
  const raw = record.providerEgressEvidence;
  if (raw?.kind === "observed") return raw;
  if (raw?.kind !== "projection") {
    throw new Error("runner scenario lacks provider egress evidence");
  }
  const source = evidence.scenarios.find((item) => item.caseId === raw.sourceCaseId);
  if (!source || source === record || source.providerEgressEvidence?.kind !== "observed" ||
      source.providerEgressEvidence.receiptId !== raw.sourceReceiptId) {
    throw new Error("provider egress projection does not resolve one observed receipt");
  }
  return source.providerEgressEvidence;
}

function validateBundleProviderEgressReceipts(evidence, fixture) {
  const receipts = evidence.scenarios.flatMap((record) => [
    ...(record.providerEgressEvidence?.kind === "observed"
      ? [record.providerEgressEvidence.receiptId] : []),
    ...record.recoveryProviderEgressEvidence.map((item) => item.evidence.receiptId),
  ]);
  if (new Set(receipts).size !== receipts.length) {
    throw new Error("provider egress receipt identities are reused across the evidence bundle");
  }
  const processTreeRoots = [
    ...(evidence.resourcePlateauEvidence === null
      ? [] : [evidence.resourcePlateauEvidence.processTreeRootId]),
    ...evidence.scenarios.flatMap((record) => [
      ...(record.providerEgressEvidence?.kind === "observed" ? [record.processTreeRootId] : []),
      ...record.recoveryProviderEgressEvidence.map((item) => item.processTreeRootId),
    ]),
  ];
  if (new Set(processTreeRoots).size !== processTreeRoots.length) {
    throw new Error("provider egress process-tree identities are reused across the evidence bundle");
  }
  for (const record of evidence.scenarios) {
    const negative = record.caseId === fixture.beforeModelNegativeFixture.caseId;
    if (negative) {
      if (record.scenarioId !== fixture.beforeModelNegativeFixture.baseScenarioId) {
        throw new Error("late-injection negative does not match its fixed base scenario");
      }
      if (record.providerEgressEvidence?.kind !== "projection" ||
          record.providerEgressEvidence.sourceCaseId !==
            fixture.beforeModelNegativeFixture.baseScenarioId) {
        throw new Error("late-injection negative does not project its fixed base egress receipt");
      }
    } else if (record.providerEgressEvidence?.kind !== "observed") {
      throw new Error("real runner scenario does not own an observed provider egress receipt");
    }
    resolveProviderEgressEvidence(evidence, record);
  }
}

function validateResourcePlateauIdentity(evidence) {
  const plateau = evidence.resourcePlateauEvidence;
  if (plateau.candidateId !== evidence.candidateId ||
      plateau.artifactFingerprint !== evidence.artifactFingerprint ||
      plateau.environmentFingerprint !== evidence.environmentFingerprint ||
      plateau.runnerInvocationId !== evidence.invocationId) {
    throw new Error("resource plateau evidence does not match the bundle identity");
  }
}

function validateResourcePlateauBinding(evidence, result, fixture) {
  const exceptionalState = ["unsupported", "not_run"].includes(result.disposition.state);
  if (exceptionalState) {
    if (evidence.resourcePlateauEvidence !== null ||
        result.resourcePlateauEvidenceFingerprint !== null) {
      throw new Error("unsupported/not-run runner evidence contains plateau workload");
    }
    return;
  }
  if (evidence.resourcePlateauEvidence === null ||
      result.resourcePlateauEvidenceFingerprint === null) {
    throw new Error("executed runner evidence is missing its resource plateau");
  }
  validateResourcePlateauEvidence(evidence.resourcePlateauEvidence, fixture);
  validateResourcePlateauIdentity(evidence);
  if (result.resourcePlateauEvidenceFingerprint !==
      resourcePlateauEvidenceFingerprint(evidence.resourcePlateauEvidence)) {
    throw new Error("resource plateau evidence fingerprint does not match the runner bundle");
  }
}

function validateRecoveryProviderBinding(wrapper, recovery, fixture) {
  const signal = recovery?.deliveredSignals.find((item) =>
    recovery.consumedSignalIds.includes(item.signalId)) ?? recovery?.deliveredSignals[0];
  const manifests = [fixture.effectiveConfiguration, fixture.localDerivationManifest,
    fixture.repairedRemoteManifest, fixture.outputLimitRecoveryManifest];
  const manifest = manifests.find((item) =>
    item.configurationFingerprint === wrapper.effectiveManifestFingerprint);
  if (!recovery || !signal || !manifest ||
      wrapper.effectiveManifestFingerprint !== signal.effectiveManifestFingerprint ||
      manifest.summaryProvider.providerFingerprint !== signal.providerFingerprint) {
    throw new Error("recovery provider egress evidence does not bind its case manifest/provider");
  }
}

function validateProviderEgressRunBinding(
  receipt, invocationId, processTreeRootId, observationCaseId,
) {
  if (receipt.observationCaseId !== observationCaseId) {
    throw new Error("provider egress receipt does not match its observation case");
  }
  if (receipt.runnerInvocationId !== invocationId) {
    throw new Error("provider egress receipt does not match the runner invocation");
  }
  if (receipt.processTreeRootId !== processTreeRootId) {
    throw new Error("provider egress receipt does not match its process tree");
  }
}

function validateRecoveryProviderEgressEvidence(record, result, fixture, networkTrustEvidence) {
  const scenario = fixture.scenarios.find((item) => item.scenarioId === result.scenarioId);
  const retry = scenario ? expectedRetryEvidence(scenario) : null;
  const exceptional = result.disposition.state === "unsupported" ||
    result.disposition.state === "not_run";
  const recoveryObserved = !exceptional && result.milestones.some((item) =>
    item.name === scenario?.drainCondition.terminalMilestone);
  const cases = !recoveryObserved ? [] :
    retry?.cases ?? (retry?.redirectCase ? [retry.redirectCase] : []);
  const observedCases = result.retryEvidence?.cases ??
    (result.retryEvidence?.redirectCase ? [result.retryEvidence.redirectCase] : []);
  const expectedCaseIds = cases.map((item) => item.caseId).sort();
  const actualCaseIds = record.recoveryProviderEgressEvidence.map((item) => item.caseId);
  if (!actualCaseIds.every((item, index) => index === 0 || actualCaseIds[index - 1] < item) ||
      !isDeepStrictEqual(actualCaseIds, expectedCaseIds)) {
    throw new Error("recovery provider egress evidence is duplicated, unsorted, or incomplete");
  }
  for (const wrapper of record.recoveryProviderEgressEvidence) {
    const recovery = cases.find((item) => item.caseId === wrapper.caseId);
    validateRecoveryProviderBinding(wrapper, recovery, fixture);
    const transmission = recovery.observedTransmissionEvidence;
    const runnerAttempted = wrapper.evidence.providerRequestCount > 0;
    const observedCase = observedCases.find((item) => item.caseId === wrapper.caseId);
    if (!observedCase || observedCase.providerAttempted !== runnerAttempted ||
        !isDeepStrictEqual(observedCase.observedTransmissionEvidence, transmission)) {
      throw new Error("recovery provider egress evidence does not match the candidate result");
    }
    if (runnerAttempted !== recovery.providerAttempted ||
        transmission.restrictedPayloadBytesSent !== 0 ||
        transmission.forbiddenSentinelObservationCount !== 0) {
      throw new Error("recovery provider egress evidence contradicts the fixed case outcome");
    }
    validateProviderEgressEvidence(
      wrapper.evidence, result, fixture, networkTrustEvidence, {
        manifestFingerprint: wrapper.effectiveManifestFingerprint,
        wireEvidence: {
          ...transmission,
          redirectLocationRequestCount: 0,
          redirectLocationPayloadBytesSent: 0,
          resentPayloadCount: 0,
        },
      },
    );
  }
}

function validateScenarioProviderEgress(evidence, record, result, fixture) {
  const primaryRecord = record.providerEgressEvidence?.kind === "projection"
    ? evidence.scenarios.find((item) =>
        item.caseId === record.providerEgressEvidence.sourceCaseId)
    : record;
  const primaryReceipt = resolveProviderEgressEvidence(evidence, record);
  validateProviderEgressRunBinding(
    primaryReceipt, evidence.invocationId, primaryRecord.processTreeRootId, primaryRecord.caseId,
  );
  for (const wrapper of record.recoveryProviderEgressEvidence) {
    validateProviderEgressRunBinding(
      wrapper.evidence, evidence.invocationId, wrapper.processTreeRootId, wrapper.caseId,
    );
  }
  validateProviderEgressEvidence(
    primaryReceipt, result, fixture, evidence.networkTrustEvidence,
  );
  validateRecoveryProviderEgressEvidence(
    record, result, fixture, evidence.networkTrustEvidence,
  );
}

export function validateRunnerEvidence(evidence, result, fixture, expectedInvocationId,
  expectedCaseIds = null) {
  validateNetworkTrustEvidence(evidence.networkTrustEvidence, fixture, evidence.invocationId);
  validateResourcePlateauBinding(evidence, result, fixture);
  if (result.networkTrustEvidenceFingerprint !==
      networkTrustEvidenceFingerprint(evidence.networkTrustEvidence)) {
    throw new Error("network trust evidence fingerprint does not match the runner bundle");
  }
  validateBundlePreparationIdentities(evidence);
  validateBundleProviderEgressReceipts(evidence, fixture);
  const actualCaseIds = evidence.scenarios.map((item) => item.caseId);
  if (!actualCaseIds.every((item, index) => index === 0 || actualCaseIds[index - 1] < item) ||
      (expectedCaseIds && !isDeepStrictEqual(actualCaseIds, expectedCaseIds))) {
    throw new Error("runner evidence scenarios are duplicated, unsorted, or incomplete");
  }
  if (evidence.fixtureId !== fixture.fixtureId ||
      evidence.fixtureFingerprint !== fixture.contractFingerprint ||
      evidence.candidateId !== result.candidateId ||
      evidence.invocationId !== expectedInvocationId ||
      evidence.environmentFingerprint !== result.environmentFingerprint ||
      evidence.artifactFingerprint !== result.artifactFingerprint ||
      result.runnerEvidenceFingerprint !== runnerEvidenceFingerprint(evidence)) {
    throw new Error("runner evidence identity does not match the result");
  }
  const record = evidence.scenarios.find((item) => item.caseId === result.runnerEvidenceCaseId);
  if (!record || record.scenarioId !== result.scenarioId ||
      record.resourceSampleMode !== result.resourceSampleMode) {
    throw new Error("runner evidence does not contain the result scenario");
  }
  if (record.resultObservationFingerprint !== runnerResultObservationFingerprint(result)) {
    throw new Error("result observation fingerprint does not match runner evidence");
  }
  validateScenarioProviderEgress(evidence, record, result, fixture);
  if (!isDeepStrictEqual(record.hostIdentityEvidence, result.hostIdentityEvidence) ||
      !isDeepStrictEqual(record.observedMilestones, result.milestones) ||
      !isDeepStrictEqual(record.processSamples, result.processSamples) ||
      !isDeepStrictEqual(record.latencyRuns, result.latencyEvidence.runs)) {
    throw new Error("result observations do not match runner evidence");
  }
  validateRunPreparations(record, result,
    result.disposition.state === "unsupported" || result.disposition.state === "not_run",
    fixture.samplingProtocol.processSampleIntervalMs, fixture.thresholds.maxSteadyProductProcessCount);
  return record;
}
