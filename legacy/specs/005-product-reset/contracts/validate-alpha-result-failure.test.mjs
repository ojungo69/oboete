import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

import { canonicalizeJson } from "../../../harness/schema/jcs.ts";
import { validateAgainstSchema } from "../../../harness/schema/validate.ts";
import { lineageDigest } from "./alpha-result-lineage.mjs";
import { buildRenderPayload, tokenizeRenderPayload } from "./alpha-result-render.mjs";
import { runnerEvidenceFingerprint, runnerResultObservationFingerprint } from "./alpha-runner-evidence.mjs";
import { clearProviderEgressEvidence } from "./provider-egress-test-helper.mjs";

const contractDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(contractDir, "../../..");
const validatorPath = join(contractDir, "validate-alpha-result.mjs");
const fixtureRoot = join(contractDir, "../fixtures");
const fixture = JSON.parse(readFileSync(join(fixtureRoot, "slice1-bidirectional-en-v1.json"), "utf8"));
const failure = JSON.parse(readFileSync(join(fixtureRoot, "alpha-result-v1.failure-example.json"), "utf8"));
const failureEvidence = JSON.parse(readFileSync(join(fixtureRoot, "runner-evidence/alpha-runner-evidence-v1.failure-example.json"), "utf8"));
const success = JSON.parse(readFileSync(join(fixtureRoot, "alpha-result-v1.example.json"), "utf8"));
const successEvidence = JSON.parse(readFileSync(join(fixtureRoot, "runner-evidence/alpha-runner-evidence-v1.example.json"), "utf8"));
const suiteRegression = JSON.parse(readFileSync(join(fixtureRoot, "alpha-result-v1.suite-regression.json"), "utf8"));
const suiteRegressionEvidence = JSON.parse(readFileSync(join(fixtureRoot, "runner-evidence/alpha-runner-evidence-v1.suite-regression.json"), "utf8"));
const resultSchema = JSON.parse(readFileSync(join(contractDir, "alpha-result-v1.schema.json"), "utf8"));
const evidenceRoot = mkdtempSync(join(tmpdir(), "free-mem-alpha-failure-evidence-"));
process.on("exit", () => rmSync(evidenceRoot, { recursive: true, force: true }));
let ordinal = 0;
const scenarioFor = (id) => fixture.scenarios.find((item) => item.scenarioId === id);
const suiteResultFor = (id) => structuredClone(suiteRegression.positiveResults.find((item) => item.scenarioId === id));
const successScenario = scenarioFor(success.scenarioId);
const zeroQualityFor = (scenario) => ({
  expectedInjectedItemCount: scenario.expectedInjectedItems.length,
  matchedInjectedItemCount: 0,
  expectedOmissionCount: scenario.expectedOmissions.length,
  matchedOmissionCount: 0,
  forbiddenFactCount: 0,
});
const zeroSuccessQuality = () => zeroQualityFor(successScenario);

function validate(result, evidenceTemplate = failureEvidence) {
  const evidence = structuredClone(evidenceTemplate);
  const record = evidence.scenarios.find((item) => item.caseId === result.runnerEvidenceCaseId);
  record.resultObservationFingerprint = runnerResultObservationFingerprint(result);
  result.runnerEvidenceFingerprint = runnerEvidenceFingerprint(evidence);
  const path = join(evidenceRoot, `evidence-${ordinal += 1}.json`);
  writeFileSync(path, JSON.stringify(evidence), { mode: 0o600 });
  return spawnSync(process.execPath,
    ["--experimental-strip-types", validatorPath,
      "--runner-evidence-root", evidenceRoot,
      "--runner-evidence", path,
      "--runner-invocation-id", evidence.invocationId, "--result", "-"], {
      cwd: repoRoot,
      input: JSON.stringify(result),
      encoding: "utf8",
    });
}

function assertAccepted(result, label, evidence) {
  const run = validate(result, evidence);
  assert.equal(run.status, 0, `${label}: ${run.stderr}${run.stdout}`);
}

function assertRejected(result, pattern, label, evidence) {
  const run = validate(result, evidence);
  assert.notEqual(run.status, 0, `${label}: unexpectedly accepted`);
  assert.match(`${run.stderr}${run.stdout}`, pattern, label);
}

function runnerEvidenceFor(result, template) {
  const evidence = structuredClone(template);
  const record = evidence.scenarios.find((item) => item.caseId === result.runnerEvidenceCaseId);
  record.hostIdentityEvidence = structuredClone(result.hostIdentityEvidence);
  record.observedMilestones = structuredClone(result.milestones);
  record.processSamples = structuredClone(result.processSamples);
  record.latencyRuns = structuredClone(result.latencyEvidence.runs);
  return evidence;
}

const recoverySignal = failure.retryEvidence.cases[0].deliveredSignals[0];
assert.equal(Object.hasOwn(recoverySignal, "configurationFingerprint"), false,
  "output-limit recovery retained a free-form summary configuration label");
assert.equal(recoverySignal.providerFingerprint,
  fixture.outputLimitRecoveryManifest.summaryProvider.providerFingerprint,
  "output-limit recovery signal is not provider-fingerprint bound");
assert.equal(recoverySignal.effectiveManifestFingerprint,
  fixture.outputLimitRecoveryManifest.configurationFingerprint,
  "output-limit recovery signal is not manifest-fingerprint bound");
assert.equal(failure.retryEvidence.cases[0].observedTransition.budgetAfterGrant, 1,
  "configuration activation refilled the automatic retry budget");
assert.equal(failure.retryEvidence.cases[0].observedTransition.budgetAfterAttempt, 0,
  "configuration activation did not consume its one-shot grant");

const staleRecoveryProviderFingerprint = structuredClone(failure);
staleRecoveryProviderFingerprint.retryEvidence.cases[0].deliveredSignals[0]
  .providerFingerprint = fixture.repairedRemoteManifest.summaryProvider.providerFingerprint;
assertRejected(staleRecoveryProviderFingerprint, /provider failure evidence/,
  "output-limit recovery accepted a stale provider fingerprint");

const conflictResult = suiteResultFor("runtime-unavailable-spool-recovery");
for (const [mutate, pattern, label] of [
  [(evidence) => { delete evidence.streamId; }, /streamId/,
    "identity conflict missing canonical stream"],
  [(evidence) => { delete evidence.conflictReceiptId; }, /conflictReceiptId/,
    "identity conflict missing receipt field"],
  [(evidence) => { delete evidence.conflictAttemptReceiptIds; }, /conflictAttemptReceiptIds/,
    "identity conflict missing repeated-attempt receipts"],
  [(evidence) => { evidence.conflictAttemptReceiptIds.pop(); }, /conflictAttemptReceiptIds/,
    "identity conflict has only one attempt receipt"],
  [(evidence) => { evidence.durableConflictReceiptCount = 2; }, /durableConflictReceiptCount/,
    "identity conflict persisted multiple receipts for one pair"],
  [(evidence) => { evidence.reason = "wrong_reason"; }, /\$\.reason/,
    "identity conflict wrong reason"],
  [(evidence) => { evidence.incomingDeliveryState = "committed"; },
    /incomingDeliveryState/, "identity conflict wrong state"],
  [(evidence) => { evidence.canonicalPayloadUnchanged = false; },
    /canonicalPayloadUnchanged/, "identity conflict canonical payload overwrite"],
  [(evidence) => { evidence.durableMemoryDelta = 1; },
    /durableMemoryDelta/, "identity conflict durable memory mutation"],
]) {
  const mutant = structuredClone(conflictResult);
  mutate(mutant.identityConflictEvidence);
  const issues = validateAgainstSchema(
    mutant.identityConflictEvidence, resultSchema.$defs.IdentityConflictEvidence, resultSchema,
  );
  assert.notEqual(issues.length, 0, `${label}: schema accepted mutation`);
  assert.match(JSON.stringify(issues), pattern, label);
}
const semanticConflictMismatch = structuredClone(conflictResult);
semanticConflictMismatch.identityConflictEvidence.conflictReceiptId =
  `conflict-receipt-v1:sha256:${"0".repeat(64)}`;
semanticConflictMismatch.identityConflictEvidence.conflictAttemptReceiptIds = [
  semanticConflictMismatch.identityConflictEvidence.conflictReceiptId,
  semanticConflictMismatch.identityConflictEvidence.conflictReceiptId,
];
assertRejected(semanticConflictMismatch, /identity conflict evidence/,
  "identity conflict schema-valid receipt mismatch", suiteRegressionEvidence);
const semanticConflictAttemptMismatch = structuredClone(conflictResult);
semanticConflictAttemptMismatch.identityConflictEvidence.conflictAttemptReceiptIds[1] =
  `conflict-receipt-v1:sha256:${"0".repeat(64)}`;
assertRejected(semanticConflictAttemptMismatch, /identity conflict evidence/,
  "identity conflict schema-valid attempt receipt mismatch", suiteRegressionEvidence);

const unobservedInjectionClaim = structuredClone(failure);
unobservedInjectionClaim.injectionBeforeModel = true;
assertRejected(unobservedInjectionClaim, /before-model injection marker/,
  "failure claimed unobserved before-model injection");

const fabricatedFailureCounts = structuredClone(failure);
fabricatedFailureCounts.counts.pending = 0;
fabricatedFailureCounts.counts.summaryCount = 7;
fabricatedFailureCounts.counts.durableMemoryCount = 7;
assertRejected(fabricatedFailureCounts, /scenario counts/,
  "resource failure fabricated persistence counts");

const skippedInjectionRender = structuredClone(failure);
const scenario = fixture.scenarios.find((item) => item.scenarioId === failure.scenarioId);
const payload = canonicalizeJson(
  buildRenderPayload(skippedInjectionRender, scenario, fixture, [], null),
);
skippedInjectionRender.attemptedItems = [];
skippedInjectionRender.attemptedRenderEvidence = {
  rendererId: "alpha-jcs-renderer-v1",
  utf8Payload: payload,
  tokenizerId: "deterministic-fixture-tokenizer-v1",
  tokenizerRevision: "1",
  tokenIds: tokenizeRenderPayload(payload),
};
skippedInjectionRender.attemptedRenderedBytes = Buffer.byteLength(payload, "utf8");
skippedInjectionRender.attemptedInjectedTokens =
  skippedInjectionRender.attemptedRenderEvidence.tokenIds.length;
assertRejected(skippedInjectionRender, /attempted render exists without an observed selection boundary/,
  "skipped injection claimed an attempted render");

const fabricatedDegradation = structuredClone(failure);
fabricatedDegradation.packDegradations = ["fabricated_degradation"];
assertRejected(fabricatedDegradation, /pack degradations do not match observed capabilities/,
  "resource failure fabricated a pack degradation");

const fabricatedMilestone = structuredClone(failure);
fabricatedMilestone.milestones.find((item) => item.name === "scenario_terminal").name =
  "fabricated_terminal";
assertRejected(fabricatedMilestone, /completed result milestones do not match the pinned lifecycle/,
  "resource failure fabricated a lifecycle milestone",
  runnerEvidenceFor(fabricatedMilestone, failureEvidence));

const inflatedAgentOperations = structuredClone(failure);
inflatedAgentOperations.securityDenominators.agentOperationCount = 999;
assertRejected(inflatedAgentOperations, /independent zero-tolerance safety boundary/,
  "failure inflated the Agent operation denominator");

const unorderedLatency = structuredClone(success);
const unorderedLatencyEvidence = structuredClone(successEvidence);
for (const target of [unorderedLatency.latencyEvidence.runs,
  unorderedLatencyEvidence.scenarios[0].latencyRuns]) {
  const first = target[0].captureTimings[0], second = target[0].captureTimings[1];
  [first.startMonotonicMs, second.startMonotonicMs] =
    [second.startMonotonicMs, first.startMonotonicMs];
  [first.endMonotonicMs, second.endMonotonicMs] =
    [second.endMonotonicMs, first.endMonotonicMs];
}
assertRejected(unorderedLatency, /latency run evidence does not match the pinned sampling protocol/,
  "capture timings contradicted event order", unorderedLatencyEvidence);

const stalePreparationEvidence = structuredClone(successEvidence);
const stalePreparation = stalePreparationEvidence.scenarios[0].runPreparations[0];
stalePreparation.observedAtMonotonicMs = stalePreparation.runStartedMonotonicMs -
  fixture.samplingProtocol.processSampleIntervalMs - 1;
assertRejected(structuredClone(success), /runner preparation evidence does not match latency runs/,
  "cold reset proof was stale before measurement", stalePreparationEvidence);

const delayedFirstMeasurement = structuredClone(success);
const delayedFirstMeasurementEvidence = structuredClone(successEvidence);
for (const run of [delayedFirstMeasurement.latencyEvidence.runs[0],
  delayedFirstMeasurementEvidence.scenarios[0].latencyRuns[0]]) {
  for (const timing of run.captureTimings) {
    timing.startMonotonicMs += 1000;
    timing.endMonotonicMs += 1000;
  }
  run.coldLexicalInjectionTiming.startMonotonicMs += 1000;
  run.coldLexicalInjectionTiming.endMonotonicMs += 1000;
}
delayedFirstMeasurementEvidence.scenarios[0].runPreparations[0].runFinishedMonotonicMs += 1000;
assertRejected(delayedFirstMeasurement, /runner preparation evidence does not match latency runs/,
  "first measurement was delayed after cold reset", delayedFirstMeasurementEvidence);

function timedOutAt(base, lastMilestone) {
  const result = structuredClone(base);
  const lastIndex = result.milestones.findIndex((item) => item.name === lastMilestone);
  assert.notEqual(lastIndex, -1, "fixture result does not contain its requested milestone");
  result.milestones = result.milestones.slice(0, lastIndex + 1);
  result.drain = { ...result.drain, status: "timed_out", timedOut: true };
  result.disposition = {
    state: "failed", reason: "drain_timed_out", successfulComparisonEligible: false,
  };
  result.identityConflictEvidence = null;
  result.injectionBeforeModel = null;
  result.packId = null;
  result.finalRenderEvidence = null;
  result.renderedBytes = 0;
  result.injectedTokens = 0;
  result.attemptedItems = structuredClone(result.injectedItems);
  result.attemptedRenderEvidence = null;
  result.attemptedRenderedBytes = 0;
  result.attemptedInjectedTokens = 0;
  const start = result.processSamples[0], steady = result.processSamples[1];
  result.processSamples = [];
  for (let monotonicMs = 0; monotonicMs < 30000; monotonicMs += 100) {
    result.processSamples.push({ ...(monotonicMs === 0 ? start : steady), monotonicMs });
  }
  result.processSamples.push({ ...base.processSamples.at(-1), monotonicMs: 30000 });
  return result;
}
const timedOutSuccessAt = (lastMilestone) => timedOutAt(success, lastMilestone);

function clearUnobservedSelection(result, scenario = successScenario) {
  for (const name of ["inputCandidates", "tracedCandidates", "deadlineUnprocessed",
    "admittedCandidates", "selectedItems"]) result.counts[name] = 0;
  result.counts.summaryCount = 0;
  result.counts.durableMemoryCount = 0;
  result.injectedItems = [];
  result.omittedItems = [];
  result.attemptedItems = [];
  result.attemptedRenderEvidence = null;
  result.selectionTimingEvidence = null;
  result.selectionElapsedMs = 0;
  result.packDegradations = [];
  result.quality = zeroQualityFor(scenario);
}

const timeoutBeforePersistence = timedOutSuccessAt("source_flush_requested_by_target_prompt");
clearUnobservedSelection(timeoutBeforePersistence);
assertAccepted(timeoutBeforePersistence, "timeout after provider attempt before summary commit",
  runnerEvidenceFor(timeoutBeforePersistence, successEvidence));

const timeoutBeforeCapture = timedOutSuccessAt("source_session_started");
clearUnobservedSelection(timeoutBeforeCapture);
timeoutBeforeCapture.counts.captured = 0;
timeoutBeforeCapture.counts.committed = 0;
timeoutBeforeCapture.securityDenominators = {
  ...Object.fromEntries(Object.keys(timeoutBeforeCapture.securityDenominators)
    .map((name) => [name, 0])),
  agentOperationCount: 1,
};
for (const name of Object.keys(timeoutBeforeCapture.securityEvidence))
  timeoutBeforeCapture.securityEvidence[name] = 0;
const timeoutBeforeCaptureEvidence = runnerEvidenceFor(timeoutBeforeCapture, successEvidence);
clearProviderEgressEvidence(timeoutBeforeCaptureEvidence, timeoutBeforeCapture.runnerEvidenceCaseId);
assertAccepted(timeoutBeforeCapture, "timeout before event capture",
  timeoutBeforeCaptureEvidence);

const timeoutBeforeProviderAttempt = timedOutSuccessAt("source_events_captured");
clearUnobservedSelection(timeoutBeforeProviderAttempt);
timeoutBeforeProviderAttempt.counts.committed = 0;
assertRejected(timeoutBeforeProviderAttempt, /authorization event identities/,
  "timeout before provider attempt claimed egress",
  runnerEvidenceFor(timeoutBeforeProviderAttempt, successEvidence));

const spoolScenario = scenarioFor("runtime-unavailable-spool-recovery"), spoolResult = suiteResultFor(spoolScenario.scenarioId);
const timeoutWhileSpooled = timedOutAt(spoolResult, "events_spooled");
clearUnobservedSelection(timeoutWhileSpooled, spoolScenario);
Object.assign(timeoutWhileSpooled.counts, {
  committed: 0, duplicateDeliveries: 0, summaryCount: 0, durableMemoryCount: 0,
});
timeoutWhileSpooled.securityDenominators.duplicateDeliveryAttemptCount = 0;
Object.assign(timeoutWhileSpooled.securityEvidence, {
  remoteProviderRequestCount: 0, remoteProviderPayloadCount: 0,
  credentialBytesSent: 0, payloadBytesSent: 0,
});
const timeoutWhileSpooledEvidence = runnerEvidenceFor(timeoutWhileSpooled, suiteRegressionEvidence);
clearProviderEgressEvidence(timeoutWhileSpooledEvidence, timeoutWhileSpooled.runnerEvidenceCaseId);
assertAccepted(timeoutWhileSpooled, "timeout while events remained spooled",
  timeoutWhileSpooledEvidence);

const timeoutBeforeSpoolCompletion = timedOutAt(spoolResult, "stable_batch_replayed_second_time");
clearUnobservedSelection(timeoutBeforeSpoolCompletion, spoolScenario);
Object.assign(timeoutBeforeSpoolCompletion.counts, {
  committed: 0, duplicateDeliveries: spoolResult.counts.duplicateDeliveries,
  summaryCount: 0, durableMemoryCount: 0,
});
assertRejected(timeoutBeforeSpoolCompletion, /authorization event identities/,
  "spool timeout before provider completion claimed egress",
  runnerEvidenceFor(timeoutBeforeSpoolCompletion, suiteRegressionEvidence));

const timeoutAfterSpoolCompletion = timedOutAt(spoolResult, "source_memory_drain_completed");
clearUnobservedSelection(timeoutAfterSpoolCompletion, spoolScenario);
Object.assign(timeoutAfterSpoolCompletion.counts, {
  committed: spoolResult.counts.committed, summaryCount: spoolResult.counts.summaryCount,
  durableMemoryCount: spoolResult.counts.durableMemoryCount,
});
assertAccepted(timeoutAfterSpoolCompletion, "timeout after spool provider completion",
  runnerEvidenceFor(timeoutAfterSpoolCompletion, suiteRegressionEvidence));

const earlyAttemptedRender = structuredClone(timeoutBeforePersistence);
function bindFinalRender(result, scenario) {
  const payload = canonicalizeJson(
    buildRenderPayload(result, scenario, fixture, result.injectedItems, result.packId),
  );
  result.finalRenderEvidence = {
    rendererId: "alpha-jcs-renderer-v1", utf8Payload: payload,
    tokenizerId: "deterministic-fixture-tokenizer-v1", tokenizerRevision: "1",
    tokenIds: tokenizeRenderPayload(payload),
  };
  result.attemptedRenderEvidence = "same_as_final";
  result.renderedBytes = Buffer.byteLength(payload, "utf8");
  result.attemptedRenderedBytes = result.renderedBytes;
  result.injectedTokens = result.finalRenderEvidence.tokenIds.length;
  result.attemptedInjectedTokens = result.injectedTokens;
}
const earlyPayload = canonicalizeJson(
  buildRenderPayload(earlyAttemptedRender, successScenario, fixture, [], null),
);
earlyAttemptedRender.attemptedRenderEvidence = {
  rendererId: "alpha-jcs-renderer-v1", utf8Payload: earlyPayload,
  tokenizerId: "deterministic-fixture-tokenizer-v1", tokenizerRevision: "1",
  tokenIds: tokenizeRenderPayload(earlyPayload),
};
earlyAttemptedRender.attemptedRenderedBytes = Buffer.byteLength(earlyPayload, "utf8");
earlyAttemptedRender.attemptedInjectedTokens =
  earlyAttemptedRender.attemptedRenderEvidence.tokenIds.length;
assertRejected(earlyAttemptedRender, /attempted render exists without an observed selection boundary/,
  "timeout before selection claimed an attempted render",
  runnerEvidenceFor(earlyAttemptedRender, successEvidence));

const earlyAttemptAliases = structuredClone(timeoutBeforePersistence);
earlyAttemptAliases.attemptedItems = "same_as_final";
earlyAttemptAliases.attemptedRenderEvidence = "same_as_final";
assertRejected(earlyAttemptAliases, /attempted render aliases require an observed final pack/,
  "timeout before selection used final-pack aliases",
  runnerEvidenceFor(earlyAttemptAliases, successEvidence));

const timeoutAfterSelection = timedOutSuccessAt("target_selection_finished");
const timeoutAfterSelectionEvidence = runnerEvidenceFor(timeoutAfterSelection, successEvidence);
assertAccepted(timeoutAfterSelection, "timeout after completed selection",
  timeoutAfterSelectionEvidence);
const postTimeoutMilestone = structuredClone(timeoutAfterSelection);
postTimeoutMilestone.milestones.find((item) => item.name === "target_selection_started").monotonicMs =
  31000;
postTimeoutMilestone.milestones.find((item) => item.name === "target_selection_finished").monotonicMs =
  31100;
postTimeoutMilestone.selectionTimingEvidence = {
  startMonotonicMs: 31000, endMonotonicMs: 31100,
};
postTimeoutMilestone.selectionElapsedMs = 100;
assertRejected(postTimeoutMilestone, /timed-out milestone occurred after the pinned timeout/,
  "timeout record contained a post-expiration milestone",
  runnerEvidenceFor(postTimeoutMilestone, successEvidence));
const postTimeoutCleanup = structuredClone(timeoutAfterSelection);
postTimeoutCleanup.processSamples.at(-1).processCount = 2;
postTimeoutCleanup.processSamples.push({ ...success.processSamples.at(-1), monotonicMs: 30100 });
assertRejected(postTimeoutCleanup, /timed-out resource sample does not match the deadline boundary/,
  "post-timeout cleanup hid live processes at the deadline",
  runnerEvidenceFor(postTimeoutCleanup, successEvidence));
const erasedSelection = structuredClone(timeoutAfterSelection);
for (const name of ["inputCandidates", "tracedCandidates", "deadlineUnprocessed",
  "admittedCandidates", "selectedItems"]) erasedSelection.counts[name] = 0;
erasedSelection.injectedItems = [];
erasedSelection.omittedItems = [];
erasedSelection.attemptedItems = [];
erasedSelection.quality = zeroSuccessQuality();
assertRejected(erasedSelection, /completed selection input count does not match the scenario/,
  "timeout erased an observed completed selection", timeoutAfterSelectionEvidence);

function observeElapsedDeadline(result) {
  for (const [name, monotonicMs] of [["target_selection_started", 701],
    ["target_selection_finished", 1451], ["target_injection_acknowledged", 1551],
    ["target_model_request_dispatched", 1651], ["scenario_terminal", 1751],
    ["post_teardown_grace_elapsed", 1851]]) {
    result.milestones.find((item) => item.name === name).monotonicMs = monotonicMs;
  }
  result.selectionTimingEvidence = { startMonotonicMs: 701, endMonotonicMs: 1451 };
  result.selectionElapsedMs = 750;
  for (let monotonicMs = 1400; monotonicMs <= 1900; monotonicMs += 100) {
    result.processSamples.push({ ...success.processSamples.at(-1), monotonicMs });
  }
}

function markDeadlineFailure(result) {
  result.counts.tracedCandidates = 0;
  result.counts.deadlineUnprocessed = result.counts.inputCandidates;
  result.counts.admittedCandidates = 0;
  result.counts.selectedItems = 0;
  result.injectedItems = [];
  result.omittedItems = [];
  result.attemptedItems = [];
  result.attemptedRenderEvidence = null;
  result.attemptedRenderedBytes = 0;
  result.attemptedInjectedTokens = 0;
  result.packId = null;
  result.finalRenderEvidence = null;
  result.renderedBytes = 0;
  result.injectedTokens = 0;
  result.quality = zeroSuccessQuality();
  result.disposition = {
    state: "failed", reason: "selection_deadline_exceeded", successfulComparisonEligible: false,
  };
}

const prematureDeadline = structuredClone(success);
markDeadlineFailure(prematureDeadline);
assertRejected(prematureDeadline, /selection left unprocessed candidates before its deadline/,
  "selection abandoned candidates before the deadline",
  runnerEvidenceFor(prematureDeadline, successEvidence));

const deadlineExceeded = structuredClone(success);
observeElapsedDeadline(deadlineExceeded);
markDeadlineFailure(deadlineExceeded);
assertAccepted(deadlineExceeded, "completed selection deadline failure",
  runnerEvidenceFor(deadlineExceeded, successEvidence));

const deadlineTimeout = timedOutSuccessAt("target_selection_finished");
deadlineTimeout.milestones.find((item) => item.name === "target_selection_started").monotonicMs = 701;
deadlineTimeout.milestones.find((item) => item.name === "target_selection_finished").monotonicMs = 1451;
deadlineTimeout.selectionTimingEvidence = { startMonotonicMs: 701, endMonotonicMs: 1451 };
deadlineTimeout.selectionElapsedMs = 750;
markDeadlineFailure(deadlineTimeout);
deadlineTimeout.disposition = { state: "failed", reason: "drain_timed_out", successfulComparisonEligible: false };
assertAccepted(deadlineTimeout, "selection deadline followed by drain timeout",
  runnerEvidenceFor(deadlineTimeout, successEvidence));

const elapsedDeadlineWithoutInputs = structuredClone(deadlineExceeded);
elapsedDeadlineWithoutInputs.counts.inputCandidates = 0;
elapsedDeadlineWithoutInputs.counts.deadlineUnprocessed = 0;
assertRejected(elapsedDeadlineWithoutInputs,
  /completed selection input count does not match the scenario/,
  "elapsed deadline failure erased available inputs",
  runnerEvidenceFor(elapsedDeadlineWithoutInputs, successEvidence));

const qualityFailure = structuredClone(success);
qualityFailure.injectedItems[0].fact = "Fixture-mismatched selected fact.";
qualityFailure.quality.matchedInjectedItemCount -= 1;
bindFinalRender(qualityFailure, successScenario);
qualityFailure.disposition = {
  state: "failed", reason: "quality_threshold_exceeded", successfulComparisonEligible: false,
};
assertAccepted(qualityFailure, "completed selection quality failure",
  runnerEvidenceFor(qualityFailure, successEvidence));

const concealedForbidden = structuredClone(qualityFailure);
concealedForbidden.injectedItems[0].fact = `prefix ${successScenario.forbiddenFacts[0]} suffix`;
bindFinalRender(concealedForbidden, successScenario);
assertRejected(concealedForbidden, /result quality counters do not match the recorded items/,
  "quality failure concealed a contained forbidden fact", runnerEvidenceFor(concealedForbidden, successEvidence));

const overlappingAnchor = structuredClone(qualityFailure), overlap = overlappingAnchor.injectedItems[1];
overlap.sourceEventIds = ["claude-to-codex-event-1"];
overlap.sourceSpans = [{ eventId: "claude-to-codex-event-1", startByte: 1, endByte: 44 }];
overlap.lineageId = lineageDigest(successScenario.sourceRepositoryScope, overlap.sourceSpans);
overlappingAnchor.quality.matchedInjectedItemCount = 2;
bindFinalRender(overlappingAnchor, successScenario);
assertRejected(overlappingAnchor, /result trace contains overlapping active source anchors/,
  "quality failure retained overlapping source anchors", runnerEvidenceFor(overlappingAnchor, successEvidence));

const duplicateIdentityTrace = structuredClone(success);
duplicateIdentityTrace.injectedItems = Array.from({ length: 4 }, () => structuredClone(success.injectedItems[0]));
duplicateIdentityTrace.quality.matchedInjectedItemCount = 1;
bindFinalRender(duplicateIdentityTrace, successScenario);
duplicateIdentityTrace.disposition = { state: "failed", reason: "quality_threshold_exceeded",
  successfulComparisonEligible: false };
assertRejected(duplicateIdentityTrace, /result trace contains duplicate active identities/,
  "quality failure duplicated one active identity",
  runnerEvidenceFor(duplicateIdentityTrace, successEvidence));

const sentinelMetadata = suiteResultFor("mixed-sensitivity-remote-projection");
const sentinelScenario = scenarioFor(sentinelMetadata.scenarioId);
sentinelMetadata.injectedItems[0].memoryId += sentinelScenario.securityOracle.forbiddenSentinels[2];
sentinelMetadata.quality.matchedInjectedItemCount = 0;
bindFinalRender(sentinelMetadata, sentinelScenario);
sentinelMetadata.disposition = { state: "failed", reason: "quality_threshold_exceeded", successfulComparisonEligible: false };
assertRejected(sentinelMetadata, /independent zero-tolerance safety boundary/,
  "final pack concealed a forbidden sentinel in metadata", suiteRegressionEvidence);

const sentinelWrapper = suiteResultFor("mixed-sensitivity-remote-projection");
sentinelWrapper.packId += sentinelScenario.securityOracle.forbiddenSentinels[2];
bindFinalRender(sentinelWrapper, sentinelScenario);
assertRejected(sentinelWrapper, /independent zero-tolerance safety boundary/,
  "final pack concealed a forbidden sentinel in wrapper metadata", suiteRegressionEvidence);

const restrictedInjection = suiteResultFor("local-derived-memory-remote-injection-rejected");
const restrictedScenario = scenarioFor(restrictedInjection.scenarioId);
const { reason: _restrictedReason, ...restrictedItem } = restrictedInjection.omittedItems[0];
restrictedItem.selectionReason = restrictedItem.sourceLane;
restrictedInjection.injectedItems = [restrictedItem];
restrictedInjection.omittedItems = [];
restrictedInjection.counts.admittedCandidates = 1;
restrictedInjection.counts.selectedItems = 1;
restrictedInjection.quality.matchedOmissionCount = 0;
restrictedInjection.quality.forbiddenFactCount = 1;
bindFinalRender(restrictedInjection, restrictedScenario);
restrictedInjection.disposition = {
  state: "failed", reason: "quality_threshold_exceeded", successfulComparisonEligible: false,
};
assertRejected(restrictedInjection, /independent zero-tolerance safety boundary/,
  "remote final pack injected a restricted item", suiteRegressionEvidence);

const relabeledRestrictedInjection = structuredClone(restrictedInjection);
relabeledRestrictedInjection.injectedItems[0].sensitivity = "eligible";
bindFinalRender(relabeledRestrictedInjection, restrictedScenario);
assertRejected(relabeledRestrictedInjection,
  /result trace provenance does not match scenario source events/,
  "remote final pack weakened source-derived sensitivity", suiteRegressionEvidence);

const crossScopeInjection = suiteResultFor("incompatible-scope-injection-rejected");
const crossScopeScenario = scenarioFor(crossScopeInjection.scenarioId);
const { reason: _crossScopeReason, ...crossScopeItem } = crossScopeInjection.omittedItems[0];
crossScopeItem.selectionReason = crossScopeItem.sourceLane;
crossScopeInjection.injectedItems = [crossScopeItem];
crossScopeInjection.omittedItems = [];
crossScopeInjection.counts.admittedCandidates = 1;
crossScopeInjection.counts.selectedItems = 1;
crossScopeInjection.quality.matchedOmissionCount = 0;
crossScopeInjection.quality.forbiddenFactCount = 1;
bindFinalRender(crossScopeInjection, crossScopeScenario);
crossScopeInjection.disposition = {
  state: "failed", reason: "quality_threshold_exceeded", successfulComparisonEligible: false,
};
assertRejected(crossScopeInjection, /independent zero-tolerance safety boundary/,
  "final pack injected a cross-repository item", suiteRegressionEvidence);

const unsupportedDeadlineOmission = structuredClone(deadlineExceeded);
const { selectionReason: _selectionReason, ...deadlineOmission } = success.injectedItems[0];
unsupportedDeadlineOmission.omittedItems = [{ ...deadlineOmission, reason: "candidate_limit" }];
unsupportedDeadlineOmission.counts.tracedCandidates = 1;
unsupportedDeadlineOmission.counts.deadlineUnprocessed = 3;
unsupportedDeadlineOmission.quality = zeroSuccessQuality();
assertRejected(unsupportedDeadlineOmission, /omission reason outside the Slice 1 contract/,
  "deadline failure used a Slice 2 omission reason",
  runnerEvidenceFor(unsupportedDeadlineOmission, successEvidence));

const fabricatedDeadlineTrace = structuredClone(deadlineExceeded);
const fabricatedOmission = {
  ...deadlineOmission,
  lineageId: "0".repeat(64),
  sourceEventIds: ["fabricated-event"],
  sourceSpans: [{ eventId: "fabricated-event", startByte: 0, endByte: 1 }],
  reason: "omitted_ineligible",
};
fabricatedDeadlineTrace.omittedItems = [fabricatedOmission];
fabricatedDeadlineTrace.counts.tracedCandidates = 1;
fabricatedDeadlineTrace.counts.deadlineUnprocessed = 3;
assertRejected(fabricatedDeadlineTrace, /result trace provenance does not match scenario source events/,
  "deadline failure fabricated trace provenance",
  runnerEvidenceFor(fabricatedDeadlineTrace, successEvidence));

const oversizedDeadlineAttempt = structuredClone(deadlineExceeded);
const oversizedOmission = { ...deadlineOmission, fact: "x".repeat(20000), reason: "omitted_budget" };
oversizedDeadlineAttempt.omittedItems = [oversizedOmission];
oversizedDeadlineAttempt.counts.tracedCandidates = 1;
oversizedDeadlineAttempt.counts.deadlineUnprocessed = 3;
oversizedDeadlineAttempt.counts.admittedCandidates = 1;
const { reason: _reason, ...oversizedAttemptedItem } = oversizedOmission;
oversizedAttemptedItem.selectionReason = oversizedAttemptedItem.sourceLane;
oversizedDeadlineAttempt.attemptedItems = [oversizedAttemptedItem];
const oversizedAttemptPayload = canonicalizeJson(
  buildRenderPayload(oversizedDeadlineAttempt, successScenario, fixture,
    oversizedDeadlineAttempt.attemptedItems, null),
);
oversizedDeadlineAttempt.attemptedRenderEvidence = {
  rendererId: "alpha-jcs-renderer-v1", utf8Payload: oversizedAttemptPayload,
  tokenizerId: "deterministic-fixture-tokenizer-v1", tokenizerRevision: "1",
  tokenIds: tokenizeRenderPayload(oversizedAttemptPayload),
};
oversizedDeadlineAttempt.attemptedRenderedBytes = Buffer.byteLength(oversizedAttemptPayload, "utf8");
oversizedDeadlineAttempt.attemptedInjectedTokens =
  oversizedDeadlineAttempt.attemptedRenderEvidence.tokenIds.length;
oversizedDeadlineAttempt.quality = zeroSuccessQuality();
assert.ok(oversizedDeadlineAttempt.attemptedRenderedBytes >
  fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.maxRenderedBytes);
assertRejected(oversizedDeadlineAttempt, /oversized attempted InjectionPack has no valid final output/,
  "deadline failure retained an oversized attempted pack",
  runnerEvidenceFor(oversizedDeadlineAttempt, successEvidence));

console.log("Alpha result failed-record invariant checks passed.");
