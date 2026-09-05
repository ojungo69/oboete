import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath } from "node:url";

import { canonicalizeJson, readIJsonFile } from "../../../harness/schema/jcs.ts";
import { validateAgainstSchema } from "../../../harness/schema/validate.ts";
import { validateArtifact } from "./alpha-result-artifact.mjs";
import { validateOutputLimitAtomicity } from "./alpha-result-atomicity.mjs";
import { readBoundedIJsonFile, readBoundedIJsonStdin } from "./alpha-result-input.mjs";
import { evaluateLatencyEvidence } from "./alpha-result-latency.mjs";
import { readRunnerEvidenceFile, validateRunnerEvidence } from "./alpha-runner-evidence.mjs";
import {
  evaluateResourceEvidence,
  validateResourcePlateauEvidence,
} from "./alpha-result-resource.mjs";
import { assertRetryEvidenceConsistent, expectedRetryEvidence } from "./alpha-result-retry.mjs";
import { evaluateSecurityEvidence } from "./alpha-result-security.mjs";
import { validateRenderEvidence } from "./alpha-result-render.mjs";
import { validateSelectionTiming } from "./alpha-result-selection.mjs";

const contractDir = dirname(fileURLToPath(import.meta.url));
const schemaPath = join(contractDir, "alpha-result-v1.schema.json");
const semanticPath = join(contractDir, "alpha-result-v1.semantic.jq");
const runnerEvidenceSchemaPath = join(contractDir, "alpha-runner-evidence-v1.schema.json");
const fixtureSchemaPath = join(contractDir, "../fixtures/slice1-bidirectional-en-v1.schema.json");
const fixtureValidatorPath = join(contractDir, "../fixtures/validate-slice1-fixture.mjs");
const defaultFixturePath = join(contractDir, "../fixtures/slice1-bidirectional-en-v1.json");
const defaultResultPath = join(contractDir, "../fixtures/alpha-result-v1.example.json");
const defaultArtifactRoot = join(contractDir, "../fixtures/artifacts");
const defaultRunnerEvidenceRoot = join(contractDir, "../fixtures/runner-evidence");
const defaultRunnerEvidencePath = join(defaultRunnerEvidenceRoot, "alpha-runner-evidence-v1.example.json");
const defaultRunnerInvocationId = "candidate-example-v1:fixture-invocation-v1";
const args = process.argv.slice(2);
const fingerprint = (domain, value) =>
  `sha256:${createHash("sha256").update(domain).update(canonicalizeJson(value)).digest("hex")}`;

if (args.length === 1 && args[0] === "--help") {
  console.log(
    "Usage: validate-alpha-result.mjs [--fixture PATH] [--artifact-root PATH] [--runner-evidence-root PATH] [--runner-evidence PATH] [--runner-invocation-id ID] [--result PATH]",
    "       validate-alpha-result.mjs [common options] --runner-evidence PATH --runner-invocation-id ID --result PATH... --negative-result PATH  # suite mode",
  );
  process.exit(0);
}

let fixturePath = defaultFixturePath, artifactRoot = defaultArtifactRoot;
let runnerEvidenceRoot = defaultRunnerEvidenceRoot, runnerEvidencePath = defaultRunnerEvidencePath;
let runnerInvocationId = defaultRunnerInvocationId;
const resultPaths = [], negativeResultPaths = [];
let fixtureSeen = false, artifactRootSeen = false, runnerEvidenceRootSeen = false;
let runnerEvidenceSeen = false, runnerInvocationSeen = false, resultSeen = false;
for (let index = 0; index < args.length; index += 2) {
  const flag = args[index];
  const value = args[index + 1];
  if (
    !value ||
    !["--fixture", "--artifact-root", "--runner-evidence-root", "--runner-evidence",
      "--runner-invocation-id",
      "--result", "--negative-result"].includes(flag) ||
    (flag === "--fixture" && fixtureSeen) ||
    (flag === "--artifact-root" && artifactRootSeen) ||
    (flag === "--runner-evidence-root" && runnerEvidenceRootSeen) ||
    (flag === "--runner-evidence" && runnerEvidenceSeen) ||
    (flag === "--runner-invocation-id" && runnerInvocationSeen)
  ) {
    throw new Error("invalid arguments; use --help for usage");
  }
  if (flag === "--fixture") {
    fixtureSeen = true;
    fixturePath = resolve(value);
  }
  if (flag === "--artifact-root") {
    artifactRootSeen = true;
    artifactRoot = resolve(value);
  }
  if (flag === "--runner-evidence-root") {
    runnerEvidenceRootSeen = true;
    runnerEvidenceRoot = resolve(value);
  }
  if (flag === "--runner-evidence") {
    runnerEvidenceSeen = true;
    runnerEvidencePath = resolve(value);
  }
  if (flag === "--runner-invocation-id") {
    runnerInvocationSeen = true;
    runnerInvocationId = value;
  }
  if (flag === "--result") {
    resultSeen = true;
    resultPaths.push(value === "-" ? value : resolve(value));
  }
  if (flag === "--negative-result") negativeResultPaths.push(value === "-" ? value : resolve(value));
}
if (resultPaths.length === 0 && negativeResultPaths.length === 0) resultPaths.push(defaultResultPath);
if (negativeResultPaths.length > 0 && resultPaths.length === 0) throw new Error("a negative result requires the complete positive suite");
if (resultSeen && !runnerEvidenceSeen) throw new Error("an explicit result requires runner evidence");
if (resultSeen && !runnerInvocationSeen) throw new Error("an explicit result requires a runner invocation ID");

execFileSync(
  process.execPath,
  ["--experimental-strip-types", fixtureValidatorPath, "--fixture", fixturePath],
  { stdio: "inherit" },
);

if (resultPaths.length > 1 || negativeResultPaths.length > 0) {
  const suiteFixture = readIJsonFile(fixturePath);
  if (resultPaths.length !== suiteFixture.scenarios.length || negativeResultPaths.length !== 1) {
    throw new Error("candidate suite has an invalid positive or negative result count");
  }
  if ([...resultPaths, ...negativeResultPaths].includes("-")) {
    throw new Error("suite result paths must be regular files");
  }
  const maxResultBytes = suiteFixture.resultLimits.maxResultBytes;
  const readResult = (path) => readBoundedIJsonFile(path, maxResultBytes);
  const suiteResults = resultPaths.map(readResult), negativeResults = negativeResultPaths.map(readResult);
  const expectedScenarioIds = suiteFixture.scenarios.map((item) => item.scenarioId).sort();
  const expectedRunnerCaseIds = [
    ...expectedScenarioIds, suiteFixture.beforeModelNegativeFixture.caseId,
  ].sort();
  const actualScenarioIds = suiteResults.map((item) => item.scenarioId).sort();
  const first = suiteResults[0];
  const runnerEvidenceSchema = readIJsonFile(runnerEvidenceSchemaPath);
  const runnerEvidence = readRunnerEvidenceFile(runnerEvidencePath, runnerEvidenceRoot, artifactRoot);
  const runnerEvidenceIssues = validateAgainstSchema(
    runnerEvidence, runnerEvidenceSchema, runnerEvidenceSchema,
  );
  if (runnerEvidenceIssues.length > 0) throw new Error("runner evidence does not match its schema");
  const negativeContract = suiteFixture.beforeModelNegativeFixture;
  const baseResult = suiteResults.find((item) => item.scenarioId === negativeContract.baseScenarioId);
  const expectedNegative = baseResult && Array.isArray(baseResult.milestones)
    ? structuredClone(baseResult)
    : null;
  if (expectedNegative) {
    const [dispatchName, injectionName] = negativeContract.nonBeforeModelMilestones;
    const injection = expectedNegative.milestones.find((item) => item.name === injectionName);
    const dispatch = expectedNegative.milestones.find((item) => item.name === dispatchName);
    if (injection && dispatch) {
      const injectionIndex = expectedNegative.milestones.indexOf(injection);
      const dispatchIndex = expectedNegative.milestones.indexOf(dispatch);
      const earlierIndex = Math.min(injectionIndex, dispatchIndex);
      const laterIndex = Math.max(injectionIndex, dispatchIndex);
      const earlierTime = Math.min(injection.monotonicMs, dispatch.monotonicMs);
      const laterTime = Math.max(injection.monotonicMs, dispatch.monotonicMs);
      expectedNegative.milestones[earlierIndex] = {
        name: dispatchName, monotonicMs: earlierTime,
      };
      expectedNegative.milestones[laterIndex] = {
        name: injectionName, monotonicMs: laterTime,
      };
    }
    expectedNegative.injectionBeforeModel = negativeContract.injectionBeforeModel;
    expectedNegative.disposition = negativeContract.expectedDisposition;
    expectedNegative.runnerEvidenceCaseId = negativeContract.caseId;
  }
  const commonIdentity = (item) =>
    item.candidateId === first.candidateId &&
    item.fixtureFingerprint === first.fixtureFingerprint &&
    item.environmentFingerprint === first.environmentFingerprint &&
    item.artifactFingerprint === first.artifactFingerprint &&
    item.runnerEvidenceFingerprint === first.runnerEvidenceFingerprint;
  if (
    !isDeepStrictEqual(actualScenarioIds, expectedScenarioIds) ||
    !suiteResults.every((item) => commonIdentity(item) && item.disposition.successfulComparisonEligible) ||
    negativeResults.length !== 1 ||
    !isDeepStrictEqual(negativeResults[0], expectedNegative)
  ) {
    throw new Error("candidate suite or required negative result is incomplete or inconsistent");
  }
  for (const item of [...suiteResults, ...negativeResults]) {
    validateRunnerEvidence(
      runnerEvidence, item, suiteFixture, runnerInvocationId, expectedRunnerCaseIds,
    );
  }
  for (const item of [...suiteResults, ...negativeResults]) {
    execFileSync(
      process.execPath,
      ["--experimental-strip-types", fileURLToPath(import.meta.url), "--fixture", fixturePath,
        "--artifact-root", artifactRoot, "--runner-evidence-root", runnerEvidenceRoot,
        "--runner-evidence", runnerEvidencePath, "--runner-invocation-id", runnerInvocationId,
        "--result", "-"],
      { input: JSON.stringify(item), stdio: ["pipe", "inherit", "inherit"] },
    );
  }
  process.exit(0);
}

const schema = readIJsonFile(schemaPath), fixtureSchema = readIJsonFile(fixtureSchemaPath);
const runnerEvidenceSchema = readIJsonFile(runnerEvidenceSchemaPath);
const resultPath = resultPaths[0];
const fixture = readIJsonFile(fixturePath);
const result = resultPath === "-"
  ? readBoundedIJsonStdin(fixture.resultLimits.maxResultBytes)
  : readBoundedIJsonFile(resultPath, fixture.resultLimits.maxResultBytes);
const issues = validateAgainstSchema(result, schema, schema);
if (issues.length > 0) {
  console.error(JSON.stringify(issues, null, 2));
  process.exit(1);
}
const runnerEvidence = readRunnerEvidenceFile(runnerEvidencePath, runnerEvidenceRoot, artifactRoot);
const runnerEvidenceIssues = validateAgainstSchema(
  runnerEvidence, runnerEvidenceSchema, runnerEvidenceSchema,
);
if (runnerEvidenceIssues.length > 0) {
  console.error(JSON.stringify(runnerEvidenceIssues, null, 2));
  process.exit(1);
}

// External refs are unsupported; the Shared prefix makes the materialized type boundary executable.
const resultSharedNames = Object.keys(schema.$defs).filter((name) => name.startsWith("Shared")).sort();
const fixtureSharedNames = Object.keys(fixtureSchema.$defs)
  .filter((name) => name.startsWith("Shared"))
  .sort();
if (!isDeepStrictEqual(resultSharedNames, fixtureSharedNames)) {
  throw new Error("Alpha result and fixture shared definition sets differ");
}
for (const name of resultSharedNames) {
  if (
    !isDeepStrictEqual(schema.$defs[name], fixtureSchema.$defs[name])
  ) {
    throw new Error(`Alpha result definition drifted from the fixture: ${name}`);
  }
}

const scenario = fixture.scenarios.find((item) => item.scenarioId === result.scenarioId);
const expectedExecutionEnvironment = {
  pins: fixture.pins,
  environment: fixture.environment,
};
const environmentFingerprint = fingerprint(
  "free-mem:alpha-execution-environment:v1\0",
  result.executionEnvironment,
);
validateArtifact(result, artifactRoot, fixture.pins.freeMemBaseCommit, fixture.artifactLimits);
const expectedManifestFingerprint = scenario?.derivationManifestId
  ? fixture.localDerivationManifest.configurationFingerprint
  : fixture.effectiveConfiguration.configurationFingerprint;
if (
  !scenario ||
  result.fixtureId !== fixture.fixtureId ||
  result.fixtureFingerprint !== fixture.contractFingerprint ||
  result.resourceSampleMode !== scenario.resourceSampleMode ||
  result.targetDestinationClass !== scenario.targetDestinationClass ||
  result.effectiveManifestFingerprint !== expectedManifestFingerprint ||
  !isDeepStrictEqual(result.executionEnvironment, expectedExecutionEnvironment) ||
  result.environmentFingerprint !== environmentFingerprint ||
  result.drain.drainConditionId !== scenario.drainCondition.drainConditionId ||
  result.drain.terminalMilestone !== scenario.drainCondition.terminalMilestone
) {
  throw new Error("result identity does not match the pinned fixture scenario");
}
const runnerRecord = validateRunnerEvidence(
  runnerEvidence, result, fixture, runnerInvocationId,
);
const exceptionalState = result.disposition.state === "unsupported" ||
  result.disposition.state === "not_run";
const selectionObserved = validateSelectionTiming(result, exceptionalState, scenario.expectedInjectedItems.length + scenario.expectedOmissions.length, fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.selectionTimeBudgetMs);

const expectedMilestones = [
  ...fixture.lifecycleProfiles[scenario.lifecycleProfileId],
];
if (result.runnerEvidenceCaseId === fixture.beforeModelNegativeFixture.caseId) {
  const [dispatchName, injectionName] = fixture.beforeModelNegativeFixture.nonBeforeModelMilestones;
  const injectionIndex = expectedMilestones.indexOf(injectionName);
  const dispatchIndex = expectedMilestones.indexOf(dispatchName);
  if (injectionIndex < 0 || dispatchIndex < 0) {
    throw new Error("late-injection negative milestones are absent from the lifecycle profile");
  }
  [expectedMilestones[injectionIndex], expectedMilestones[dispatchIndex]] =
    [dispatchName, injectionName];
}
const milestoneNames = result.milestones.map((item) => item.name);
const drainMilestoneTimes = new Map(result.milestones.map((item) => [item.name, item.monotonicMs]));
if (result.runnerEvidenceCaseId === fixture.beforeModelNegativeFixture.caseId) {
  const [dispatchName, injectionName] = fixture.beforeModelNegativeFixture.nonBeforeModelMilestones;
  const dispatchIndex = milestoneNames.indexOf(dispatchName);
  const injectionIndex = milestoneNames.indexOf(injectionName);
  if (
    dispatchIndex < 0 || injectionIndex < 0 || dispatchIndex >= injectionIndex ||
    drainMilestoneTimes.get(dispatchName) >= drainMilestoneTimes.get(injectionName) ||
    result.injectionBeforeModel !== false
  ) {
    throw new Error(
      "late-injection negative must dispatch strictly before injection acknowledgment",
    );
  }
}
if (result.milestones.some((item, index) =>
  index > 0 && item.monotonicMs <= result.milestones[index - 1].monotonicMs
)) {
  throw new Error("result milestone times are not strictly increasing");
}
const drainStartTime = drainMilestoneTimes.get(scenario.drainCondition.startMilestone);
const drainTerminalTime = drainMilestoneTimes.get(scenario.drainCondition.terminalMilestone);
if (
  !result.drain.timedOut &&
  typeof drainStartTime === "number" &&
  typeof drainTerminalTime === "number" &&
  drainTerminalTime - drainStartTime >= scenario.drainCondition.timeoutMs
) {
  throw new Error("completed drain reached or exceeded the pinned timeout");
}
const lastObservationTime = result.processSamples.at(-1)?.monotonicMs;
if (result.drain.timedOut && typeof drainStartTime === "number" && result.milestones.some((item) => item.monotonicMs > drainStartTime + scenario.drainCondition.timeoutMs)) throw new Error("timed-out milestone occurred after the pinned timeout");
if (
  result.drain.timedOut &&
  (typeof drainStartTime !== "number" ||
    typeof lastObservationTime !== "number" ||
    lastObservationTime - drainStartTime < scenario.drainCondition.timeoutMs)
) {
  throw new Error("timed-out result did not reach the pinned timeout");
}
const milestonesPass = result.drain.timedOut
  ? milestoneNames.length < expectedMilestones.length &&
    milestoneNames.every((name, index) => name === expectedMilestones[index]) &&
    !milestoneNames.includes(scenario.drainCondition.terminalMilestone)
  : isDeepStrictEqual(milestoneNames, expectedMilestones);
if (!exceptionalState && !milestonesPass) throw new Error(result.drain.timedOut
  ? "timed-out result milestones are not a valid pre-terminal lifecycle prefix"
  : "completed result milestones do not match the pinned lifecycle");
const selectionFinishedObserved = milestoneNames.includes("target_selection_finished");
const expectedDegradations = scenario.drainCondition.targetInjectionAcknowledged &&
    selectionFinishedObserved && fixture.effectiveConfiguration.embeddingProvider.state === "disabled"
  ? [fixture.effectiveConfiguration.embeddingProvider.packDegradationReason] : [];
if (!isDeepStrictEqual(result.packDegradations, expectedDegradations)) throw new Error("pack degradations do not match observed capabilities");
const injectionAcknowledgedAt = drainMilestoneTimes.get("target_injection_acknowledged");
const modelDispatchedAt = drainMilestoneTimes.get("target_model_request_dispatched");
const expectedInjectionBeforeModel = scenario.drainCondition.targetInjectionAcknowledged &&
  !result.drain.timedOut &&
  typeof injectionAcknowledgedAt === "number" &&
  typeof modelDispatchedAt === "number"
  ? injectionAcknowledgedAt < modelDispatchedAt
  : null;
if (result.injectionBeforeModel !== expectedInjectionBeforeModel) {
  throw new Error("before-model injection marker does not match observed milestones");
}
const expectedHostIdentityEvidence = !exceptionalState &&
  result.scenarioId === fixture.hostIdentityProbe.scenarioId
  ? fixture.hostIdentityProbe.expectedResult
  : null;
if (!isDeepStrictEqual(result.hostIdentityEvidence, expectedHostIdentityEvidence)) {
  throw new Error("host-derived identity evidence does not match the pinned claim probes");
}
const drainTerminalObserved = milestoneNames.includes(scenario.drainCondition.terminalMilestone);
const operationalStatusObserved = milestoneNames.includes("operational_status_inspected");
const expectedOperationalStatus = operationalStatusObserved ? scenario.expectedOperationalStatus ?? null : null;
const expectedFailureMetadata = drainTerminalObserved ? scenario.fault?.failureMetadata ?? null : null;
validateOutputLimitAtomicity(result, scenario, exceptionalState);
const expectedRetryEvidenceRecord = exceptionalState || !drainTerminalObserved ? null : expectedRetryEvidence(scenario);
if (!isDeepStrictEqual(result.failureMetadata, expectedFailureMetadata) ||
    !isDeepStrictEqual(result.operationalStatus, expectedOperationalStatus) ||
    !isDeepStrictEqual(result.retryEvidence, expectedRetryEvidenceRecord)) {
  throw new Error("provider failure evidence does not match observed lifecycle");
}
const conflictProbe = scenario.fault?.identityConflictProbe;
const expectedIdentityConflictEvidence = conflictProbe && drainTerminalObserved
  ? {
      repositoryScope: conflictProbe.repositoryScope,
      source: conflictProbe.source,
      streamId: conflictProbe.streamId,
      eventId: conflictProbe.eventId,
      payloadDigestVersion: conflictProbe.payloadDigestVersion,
      canonicalPayloadDigest: conflictProbe.canonicalPayloadDigest,
      conflictingPayloadDigest: conflictProbe.conflictingPayloadDigest,
      conflictReceiptId: conflictProbe.conflictReceiptId,
      conflictAttemptReceiptIds: conflictProbe.conflictAttemptReceiptIds,
      durableConflictReceiptCount: conflictProbe.durableConflictReceiptCount,
      conflictReceiptState: conflictProbe.conflictReceiptState,
      canonicalEventState: conflictProbe.canonicalEventState,
      incomingDeliveryState: conflictProbe.incomingDeliveryState,
      reason: conflictProbe.expectedReason,
      canonicalPayloadUnchanged: conflictProbe.canonicalPayloadUnchanged,
      durableMemoryDelta: conflictProbe.durableMemoryDelta,
    }
  : null;
if (!isDeepStrictEqual(result.identityConflictEvidence, expectedIdentityConflictEvidence)) {
  throw new Error("identity conflict evidence does not match observed lifecycle");
}
assertRetryEvidenceConsistent(result);

const matchingPositions = (actual, expected) =>
  expected.reduce(
    (count, item, index) => count + Number(isDeepStrictEqual(actual[index], item)),
    0,
  );
const derivedQuality = {
  expectedInjectedItemCount: scenario.expectedInjectedItems.length,
  matchedInjectedItemCount: matchingPositions(result.injectedItems, scenario.expectedInjectedItems),
  expectedOmissionCount: scenario.expectedOmissions.length,
  matchedOmissionCount: matchingPositions(result.omittedItems, scenario.expectedOmissions),
  forbiddenFactCount: result.injectedItems.filter((item) =>
    scenario.forbiddenFacts.some((forbidden) => item.fact.includes(forbidden))
  ).length,
};
if (!isDeepStrictEqual(result.quality, derivedQuality)) {
  throw new Error("result quality counters do not match the recorded items");
}
const qualityPass =
  isDeepStrictEqual(result.injectedItems, scenario.expectedInjectedItems) &&
  isDeepStrictEqual(result.omittedItems, scenario.expectedOmissions) &&
  derivedQuality.forbiddenFactCount === 0;

const expectedDuplicateDeliveries = scenario.fault?.replaySchedule
  ? scenario.fault.replaySchedule.slice(1).reduce((count, item) => count + item.eventIds.length, 0)
  : 0;
const captureObserved = !result.drain.timedOut || milestoneNames.includes("source_events_captured") || milestoneNames.includes("source_events_accepted");
const duplicateReplayObserved = !result.drain.timedOut || milestoneNames.includes("stable_batch_replayed_second_time");
const persistenceMilestone = ["source_summary_committed", "source_memory_drain_completed", "local_provider_derived_memory"].find((name) => expectedMilestones.includes(name));
const commitMilestone = expectedMilestones.includes("source_flush_requested_by_target_prompt") ? "source_flush_requested_by_target_prompt" : persistenceMilestone ?? scenario.drainCondition.terminalMilestone, commitObserved = !result.drain.timedOut || milestoneNames.includes(commitMilestone);
const persistenceObserved = !result.drain.timedOut || persistenceMilestone === undefined ||
  milestoneNames.includes(persistenceMilestone);
const pendingObserved = !result.drain.timedOut || milestoneNames.includes("source_flush_requested_by_target_prompt");
const countsPass =
  result.counts.captured === (captureObserved ? scenario.events.length : 0) &&
  result.counts.committed === (commitObserved ? scenario.drainCondition.committedEventCount : 0) &&
  result.counts.duplicateDeliveries ===
    (duplicateReplayObserved ? expectedDuplicateDeliveries : 0) &&
  result.counts.lost === scenario.expectedCounters.acceptedEventLossCount &&
  result.counts.pending === (pendingObserved
    ? (scenario.drainCondition.pendingSummaryJobCount ?? 0) : 0) &&
  result.counts.summaryCount === (persistenceObserved ? scenario.drainCondition.summaryCount : 0) &&
  result.counts.durableMemoryCount ===
    (persistenceObserved ? scenario.drainCondition.durableMemoryCount : 0);
if (!exceptionalState && !countsPass) throw new Error("scenario counts do not match the pinned lifecycle");
const tracedItems = result.injectedItems.length + result.omittedItems.length;
const admittedItems = result.injectedItems.length + result.omittedItems.filter(
  (item) => item.reason === "omitted_budget" || item.reason === "lane_minimum_not_funded",
).length;
if (
  result.counts.tracedCandidates !== tracedItems ||
  result.counts.admittedCandidates !== admittedItems ||
  result.counts.selectedItems !== result.injectedItems.length
) {
  throw new Error("candidate counters do not match the recorded item dispositions");
}

const securityEvaluation = evaluateSecurityEvidence({
  result, scenario, fixture, exceptionalState, expectedDuplicateDeliveries,
});
const { oracle, activeSummaryProvider, denominatorsPass, safetyCountersPass,
  securityEvidencePass } = securityEvaluation;

const latencyPass = evaluateLatencyEvidence(
  result, scenario, fixture, exceptionalState, runnerRecord.latencyRuns,
);

const resourcePlateauPass = exceptionalState || validateResourcePlateauEvidence(
  runnerEvidence.resourcePlateauEvidence, fixture,
);
const scenarioResourcePass = evaluateResourceEvidence(
  result, fixture, exceptionalState, runnerRecord, scenario.drainCondition.timeoutMs,
);
const resourcePass = resourcePlateauPass && scenarioResourcePass;
const injectionEnvelope = fixture.effectiveConfiguration.resourceProfile.injectionEnvelope;
const selectionDeadlineExceeded = selectionObserved &&
  result.selectionElapsedMs >= injectionEnvelope.selectionTimeBudgetMs;
if (result.drain.timedOut && selectionFinishedObserved && !selectionDeadlineExceeded && !qualityPass)
  throw new Error("completed selection does not match the pinned item trace");
const finalInjectionPackSizePass =
  result.renderedBytes <= injectionEnvelope.maxRenderedBytes &&
  result.injectedTokens <= injectionEnvelope.maxInjectedTokens;
const finalPackExpected = !exceptionalState &&
  scenario.drainCondition.targetInjectionAcknowledged && !result.drain.timedOut &&
  !selectionDeadlineExceeded;
validateRenderEvidence(result, scenario, fixture, finalPackExpected);
if (!finalInjectionPackSizePass) {
  throw new Error("oversized InjectionPack was recorded as final output");
}
if (
  selectionDeadlineExceeded &&
  (result.counts.selectedItems !== 0 ||
    result.injectedItems.length !== 0 ||
    result.renderedBytes !== 0 ||
    result.injectedTokens !== 0)
) {
  throw new Error("late InjectionPack was recorded as delivered");
}
const deterministicProviderStub =
  fixture.pins.summaryProviderStub === "deterministic-summary-v1";
const expectedProviderCostUnits = deterministicProviderStub ||
  activeSummaryProvider.costClass === "local_zero" ? 0 : null;
if (!exceptionalState && result.providerCostUnits !== expectedProviderCostUnits) {
  throw new Error("provider cost does not match the pinned deterministic provider stub evidence");
}
const scenarioOraclePass = !scenario.drainCondition.targetInjectionAcknowledged || expectedInjectionBeforeModel === true;
const derivedFailureReason = result.drain.timedOut
  ? "drain_timed_out"
  : selectionDeadlineExceeded
    ? "selection_deadline_exceeded"
    : !latencyPass
      ? "latency_threshold_exceeded"
      : !resourcePass
        ? "resource_threshold_exceeded"
        : !qualityPass
          ? "quality_threshold_exceeded"
          : !scenarioOraclePass
            ? "scenario_oracle_mismatch"
            : null;
const shouldBeEligible = derivedFailureReason === null;

if (!exceptionalState && !(denominatorsPass && safetyCountersPass && securityEvidencePass)) {
  throw new Error("result violates an independent zero-tolerance safety boundary");
}

if (exceptionalState) {
  const expectedReason = result.disposition.state === "unsupported"
    ? "capability_unsupported"
    : "owner_slice_not_run";
  const noActivity =
    Object.values(result.counts).every((value) => value === 0) &&
    Object.values(result.safety).every((value) => value === 0) &&
    Object.values(result.securityDenominators).every((value) => value === 0) &&
    Object.values(result.securityEvidence).every((value) => value === 0) &&
    Object.values(result.resource).every((value) => value === 0) &&
    result.injectedItems.length === 0 &&
    result.omittedItems.length === 0 &&
    result.injectionBeforeModel === null &&
    result.hostIdentityEvidence === null &&
    result.packDegradations.length === 0 &&
    Array.isArray(result.attemptedItems) &&
    result.attemptedItems.length === 0 &&
    result.attemptedRenderEvidence === null && result.finalRenderEvidence === null &&
    result.packId === null &&
    result.attemptedRenderedBytes === 0 &&
    result.renderedBytes === 0 &&
    result.attemptedInjectedTokens === 0 &&
    result.injectedTokens === 0 &&
    result.selectionElapsedMs === 0 &&
    result.providerCostUnits === null &&
    result.retryEvidence === null &&
    result.identityConflictEvidence === null &&
    result.failureMetadata === null &&
    result.outputLimitAtomicityEvidence === null &&
    result.operationalStatus === null &&
    !result.drain.timedOut &&
    result.milestones.length === 0;
  if (
    !noActivity ||
    !resourcePass ||
    result.disposition.reason !== expectedReason ||
    result.disposition.successfulComparisonEligible
  ) {
    throw new Error("unsupported/not-run disposition is not a canonical no-activity record");
  }
} else if (shouldBeEligible) {
  const expectedReason = expectedOperationalStatus?.reason ??
    oracle.expectedReason ?? expectedDegradations[0] ?? "all_thresholds_passed";
  const expectedState = expectedReason === "all_thresholds_passed" ? "healthy" : "degraded";
  if (
    !result.disposition.successfulComparisonEligible ||
    result.disposition.state !== expectedState ||
    result.disposition.reason !== expectedReason
  ) {
    throw new Error("passing evidence is not recorded as an eligible result");
  }
} else {
  if (
    (result.drain.timedOut
      ? result.disposition.state !== "failed" && result.disposition.state !== "degraded"
      : result.disposition.state !== "failed") ||
    result.disposition.reason !== derivedFailureReason ||
    result.disposition.successfulComparisonEligible
  ) {
    throw new Error("failed evidence is not recorded with the derived non-eligible reason");
  }
}

try {
  execFileSync("jq", ["-e", "-f", semanticPath], {
    input: JSON.stringify(result),
    stdio: ["pipe", "inherit", "inherit"],
  });
} catch (error) {
  if (error?.code === "ENOENT") {
    console.error("Prerequisite missing: jq is required to validate Alpha results.");
    process.exit(2);
  }
  throw error;
}
