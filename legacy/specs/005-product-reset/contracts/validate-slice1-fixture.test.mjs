import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { validateAgainstSchema } from "../../../harness/schema/validate.ts";
import { canonicalizeJson } from "../../../harness/schema/jcs.ts";

const contractDir = dirname(fileURLToPath(import.meta.url));
const fixtureRoot = join(contractDir, "../fixtures");
const semanticPath = join(fixtureRoot, "slice1-bidirectional-en-v1.semantic.jq");
const validatorPath = join(fixtureRoot, "validate-slice1-fixture.mjs");
const mutantRoot = mkdtempSync(join(tmpdir(), "free-mem-slice1-fixture-mutants-"));
let mutantOrdinal = 0;
process.on("exit", () => rmSync(mutantRoot, { recursive: true, force: true }));
const fixture = JSON.parse(readFileSync(join(fixtureRoot,
  "slice1-bidirectional-en-v1.json"), "utf8"));
const fixtureSchema = JSON.parse(readFileSync(join(fixtureRoot,
  "slice1-bidirectional-en-v1.schema.json"), "utf8"));

const fingerprint = (domain, value) => `sha256:${createHash("sha256")
  .update(domain).update(canonicalizeJson(value)).digest("hex")}`;
const providerFingerprint = (provider, domain = "free-mem:provider-choice:v1\0") => {
  const { providerFingerprint: _providerFingerprint, ...choice } = provider;
  return fingerprint(domain, choice);
};
const manifestFingerprint = (manifest,
  domain = "free-mem:effective-capability-manifest:v1\0") => {
  const { configurationFingerprint: _configurationFingerprint, ...configuration } = manifest;
  return fingerprint(domain, configuration);
};

assert.deepEqual(validateAgainstSchema(fixture, fixtureSchema, fixtureSchema), [],
  "corrected fixed fixture does not match its structural schema");

const manifests = [fixture.effectiveConfiguration, fixture.localDerivationManifest,
  fixture.repairedRemoteManifest, fixture.outputLimitRecoveryManifest];
const providerChoiceKeys = ["costClass", "credentialRef", "egressPolicy", "endpointUrl",
  "executionLocation", "modelId", "modelRevision", "providerFingerprint", "redirectPolicy",
  "role", "state", "tlsPolicy", "version", "wireProtocol"].sort();
for (const manifest of manifests) {
  assert.deepEqual(Object.keys(manifest.summaryProvider).sort(), providerChoiceKeys,
    `${manifest.manifestId}: ProviderChoice is not the closed v1 shape`);
  assert.equal(manifest.summaryProvider.providerFingerprint,
    providerFingerprint(manifest.summaryProvider),
    `${manifest.manifestId}: provider fingerprint is stale`);
  assert.equal(manifest.configurationFingerprint, manifestFingerprint(manifest),
    `${manifest.manifestId}: manifest fingerprint is stale`);
  assert.notEqual(manifest.configurationFingerprint,
    manifestFingerprint(manifest, "free-mem:effective-manifest:v1\0"),
    `${manifest.manifestId}: legacy manifest fingerprint domain was accepted`);
}
assert.equal(fixture.effectiveConfiguration.summaryProvider.endpointUrl,
  "https://summary.stub.invalid/v1/chat/completions");
assert.deepEqual(fixture.effectiveConfiguration.summaryProvider.credentialRef,
  { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" });
assert.equal(fixture.localDerivationManifest.summaryProvider.endpointUrl,
  "https://127.0.0.1:1234/v1/chat/completions");
assert.deepEqual(fixture.localDerivationManifest.summaryProvider.credentialRef, { kind: "none" });
assert.equal(new URL(fixture.localDerivationManifest.summaryProvider.endpointUrl).protocol, "https:",
  "restricted local derivation provider is not authenticated HTTPS");
assert.equal(fixture.localDerivationManifest.summaryProvider.tlsPolicy, "system",
  "restricted local derivation provider does not require verified TLS");
const credentialedLocalTransmission = structuredClone(fixture);
credentialedLocalTransmission.scenarios.find((scenario) =>
  scenario.derivationManifestId === fixture.localDerivationManifest.manifestId
).providerTransmissionOracle.credentialBytesSent = 1;
assertFixtureRejected(credentialedLocalTransmission,
  "fixture semantics accepted credentials on a credential-none local provider");
assert.equal(fixture.repairedRemoteManifest.summaryProvider.endpointUrl,
  "https://summary-repaired.stub.invalid/v1/chat/completions");
for (const name of ["maxSourceEventsPerJob", "periodicSweepIntervalMs", "idleFlushMs",
  "eventDebounceMs", "stuckClaimTimeoutMs", "rawEventRetentionEnabled",
  "rawEventRetentionMs", "observerRequestTimeoutMs", "observerMaxInputChars",
  "observerMaxOutputTokens", "observerMaxResponseBytes", "observerTemperature",
  "providerTlsPreflightTimeoutMs"]) {
  assert.ok(Object.hasOwn(fixture.effectiveConfiguration.resourceProfile, name),
    `ResourceProfile is missing ${name}`);
}

const oldProviderShape = structuredClone(fixture);
oldProviderShape.effectiveConfiguration.summaryProvider = {
  state: "enabled", providerId: "deterministic-summary-v1",
  providerKind: "explicit_remote_endpoint", modelId: "deterministic-summary-model-v1",
  modelRevision: "1", configurationFingerprint: "summary-config-v1",
  executionLocation: "remote", endpointScheme: "https", endpointHost: "summary.stub.invalid",
  credentialSource: "fixture-credential-ref", costClass: "fixture",
  egressPolicy: "explicit_remote", capabilities: ["summary_v1"], validationState: "valid",
  tlsCertificateValidation: "required", redirectPolicy: "reject",
};
assert.notEqual(validateAgainstSchema(oldProviderShape, fixtureSchema, fixtureSchema).length, 0,
  "fixture schema accepted the legacy ProviderChoice shape");

const legacyConflict = structuredClone(fixture);
legacyConflict.effectiveConfiguration.legacyDispositions = [
  { key: "summary.endpoint", disposition: "conflict" },
];
assert.notEqual(validateAgainstSchema(legacyConflict, fixtureSchema, fixtureSchema).length, 0,
  "fixture schema accepted an active legacy conflict");

function assertFixtureRejected(mutant, label) {
  const run = spawnSync("jq", ["-e", "-f", semanticPath], {
    input: JSON.stringify(mutant), encoding: "utf8",
  });
  assert.equal(run.error, undefined, `${label}: jq did not start`);
  assert.equal(typeof run.status, "number", `${label}: jq did not report an exit status`);
  assert.notEqual(run.status, 0, label);
}

function assertCanonicalFixtureRejected(mutant, pattern, label) {
  const path = join(mutantRoot, `fixture-${mutantOrdinal += 1}.json`);
  writeFileSync(path, JSON.stringify(mutant), { mode: 0o600 });
  const run = spawnSync(process.execPath,
    ["--experimental-strip-types", validatorPath, "--fixture", path], { encoding: "utf8" });
  assert.equal(run.error, undefined, `${label}: canonical validator did not start`);
  assert.equal(typeof run.status, "number",
    `${label}: canonical validator did not report an exit status`);
  assert.notEqual(run.status, 0, `${label}: canonical validator unexpectedly accepted fixture`);
  assert.match(`${run.stderr}${run.stdout}`, pattern, label);
}

const fixedConflictProbe = fixture.scenarios.find((scenario) =>
  scenario.scenarioId === "runtime-unavailable-spool-recovery").fault.identityConflictProbe;
assert.ok(fixedConflictProbe.conflictAttemptReceiptIds.length >= 2,
  "identity conflict does not include repeated attempts");
assert.ok(fixedConflictProbe.conflictAttemptReceiptIds.every((receiptId) =>
  receiptId === fixedConflictProbe.conflictReceiptId),
  "identity conflict attempts do not reuse one pair-bound receipt");
assert.equal(fixedConflictProbe.durableConflictReceiptCount, 1,
  "identity conflict does not prove one durable receipt");
assert.match(fixedConflictProbe.conflictReceiptId,
  /^conflict-receipt-v1:sha256:[0-9a-f]{64}$/u,
  "identity conflict receipt is not a pair-bound fingerprint");

const fixedSignals = fixture.scenarios.flatMap((scenario) => [
  ...(scenario.fault?.resumeCases ?? []).flatMap((item) => item.signals),
  ...(scenario.fault?.redirectRecovery?.signal ? [scenario.fault.redirectRecovery.signal] : []),
]);
assert.ok(fixedSignals.every((signal) => signal.targetJobId && signal.producerReceiptId),
  "resume signals are not bound to a job and producer receipt");

const crossPairReceiptReuse = structuredClone(fixture);
crossPairReceiptReuse.scenarios.find((scenario) =>
  scenario.scenarioId === "runtime-unavailable-spool-recovery").fault.identityConflictProbe
  .conflictingPayloadDigest = `sha256:${"0".repeat(64)}`;
assertCanonicalFixtureRejected(crossPairReceiptReuse,
  /identity-conflict receipt is not unique to and reused for one digest pair/,
  "fixture accepted one conflict receipt for a different digest pair");

const changedAttemptReceipt = structuredClone(fixture);
changedAttemptReceipt.scenarios.find((scenario) =>
  scenario.scenarioId === "runtime-unavailable-spool-recovery").fault.identityConflictProbe
  .conflictAttemptReceiptIds[1] = `conflict-receipt-v1:sha256:${"0".repeat(64)}`;
assertCanonicalFixtureRejected(changedAttemptReceipt,
  /identity-conflict receipt is not unique to and reused for one digest pair/,
  "fixture accepted a different receipt on conflict retry");

const crossStreamReceiptReuse = structuredClone(fixture);
const crossStreamScenario = crossStreamReceiptReuse.scenarios.find((scenario) =>
  scenario.scenarioId === "runtime-unavailable-spool-recovery");
crossStreamScenario.sourceStreamId = "another-source-stream";
crossStreamScenario.fault.identityConflictProbe.streamId = "another-source-stream";
assertCanonicalFixtureRejected(crossStreamReceiptReuse,
  /identity-conflict receipt is not unique to and reused for one digest pair/,
  "fixture accepted one conflict receipt across source streams");

const activationScenario = fixture.scenarios.find((scenario) =>
  Object.hasOwn(scenario, "providerActivationProposal"));
for (const [mutate, pattern, label] of [
  [(proposal) => { proposal.modelId = "界".repeat(134); }, /model.*UTF-8 byte bounds/,
    "400-byte multibyte ProviderProposal model"],
  [(proposal) => { proposal.modelRevision = "revision\u0001"; }, /model.*control/,
    "ProviderProposal model revision control character"],
  [(proposal) => { proposal.endpointUrl = "http://user@summary.http.invalid/v1/chat/completions"; },
    /userinfo/, "ProviderProposal endpoint userinfo"],
  [(proposal) => { proposal.endpointUrl = "http://summary.http.invalid/v1/chat/completions?mode=test"; },
    /query or fragment/, "ProviderProposal endpoint query"],
  [(proposal) => { proposal.endpointUrl = "http://summary.http.invalid:80/v1/chat/completions"; },
    /canonical/, "noncanonical ProviderProposal endpoint"],
  [(proposal) => { proposal.endpointUrl = "http://localhost:1234/v1/chat/completions"; },
    /hostname/, "ProviderProposal localhost endpoint"],
  [(proposal) => { proposal.endpointUrl = "http://foo.localhost/v1/chat/completions"; },
    /hostname/, "ProviderProposal localhost-subdomain endpoint"],
  [(proposal) => { proposal.endpointUrl = "http://localhost.:1234/v1/chat/completions"; },
    /trailing-dot/, "ProviderProposal localhost trailing-dot endpoint"],
  [(proposal) => { proposal.endpointUrl = "http://foo.localhost./v1/chat/completions"; },
    /trailing-dot/, "ProviderProposal localhost-subdomain trailing-dot endpoint"],
  [(proposal) => { proposal.endpointUrl = "http://summary.http.invalid./v1/chat/completions"; },
    /trailing-dot/, "ProviderProposal remote trailing-dot endpoint"],
]) {
  const mutant = structuredClone(fixture);
  const proposal = mutant.scenarios.find((scenario) =>
    scenario.scenarioId === activationScenario.scenarioId).providerActivationProposal.proposal;
  mutate(proposal);
  assertCanonicalFixtureRejected(mutant, pattern, label);
}

const unsortedLegacyDispositions = structuredClone(fixture);
unsortedLegacyDispositions.effectiveConfiguration.legacyDispositions = [
  { key: "summary.provider", disposition: "ignored" },
  { key: "summary.endpoint", disposition: "translated" },
];
assertFixtureRejected(unsortedLegacyDispositions,
  "fixture semantics accepted unsorted active legacy dispositions");

const swappedRecoveryKinds = structuredClone(fixture);
const retryCases = swappedRecoveryKinds.scenarios.find((scenario) =>
  scenario.scenarioId === "summary-provider-retry-exhausted").fault.resumeCases;
const configurationCase = retryCases.find((item) =>
  item.caseId === "validated-configuration-activation");
const doctorCase = retryCases.find((item) => item.caseId === "user-confirmed-doctor-retry");
[configurationCase.signals[0].kind, doctorCase.signals[0].kind] =
  [doctorCase.signals[0].kind, configurationCase.signals[0].kind];
assertFixtureRejected(swappedRecoveryKinds,
  "fixture semantics accepted recovery kinds swapped between case IDs");

const mismatchedSignalJob = structuredClone(fixture);
mismatchedSignalJob.scenarios.find((scenario) =>
  scenario.scenarioId === "summary-provider-retry-exhausted").fault.resumeCases[0]
  .signals[0].targetJobId = "another-job";
assertCanonicalFixtureRejected(mismatchedSignalJob, /resume signal.*job and producer receipt/,
  "fixture accepted a signal for a different explicit job");

const mismatchedDuplicateProducer = structuredClone(fixture);
const duplicateProducerSignals = mismatchedDuplicateProducer.scenarios.find((scenario) =>
  scenario.scenarioId === "summary-provider-retry-exhausted").fault.resumeCases.find((item) =>
  item.caseId === "duplicate-and-out-of-order-no-op").signals;
duplicateProducerSignals[1].producerReceiptId = "another-producer-receipt";
assertCanonicalFixtureRejected(
  mismatchedDuplicateProducer, /resume signal.*job and producer receipt/,
  "fixture accepted one signal ID with multiple producer receipts",
);

const reusedProducerReceipt = structuredClone(fixture);
const reusedProducerCases = reusedProducerReceipt.scenarios.find((scenario) =>
  scenario.scenarioId === "summary-provider-retry-exhausted").fault.resumeCases;
reusedProducerCases[1].signals[0].producerReceiptId =
  reusedProducerCases[0].signals[0].producerReceiptId;
assertCanonicalFixtureRejected(
  reusedProducerReceipt, /resume signal.*job and producer receipt/,
  "fixture accepted one producer receipt for multiple signal IDs",
);

for (const [scenarioId, recovery] of [
  ["summary-provider-redirect-rejected", (scenario) => scenario.fault.redirectRecovery],
  ["summary-provider-output-limit-exceeded", (scenario) => scenario.fault.resumeCases.find(
    (item) => item.caseId === "validated-larger-limit-activation")],
]) {
  const staleSequence = structuredClone(fixture);
  const target = recovery(staleSequence.scenarios.find((scenario) =>
    scenario.scenarioId === scenarioId));
  const signal = target.signals?.[0] ?? target.signal;
  target.expected.lastConsumedSequence = signal.sequence + 1;
  assertFixtureRejected(staleSequence,
    `fixture semantics accepted lastConsumedSequence ahead of the signal for ${scenarioId}`);

  const coupledSequenceDrift = structuredClone(fixture);
  const coupledTarget = recovery(coupledSequenceDrift.scenarios.find((scenario) =>
    scenario.scenarioId === scenarioId));
  const coupledSignal = coupledTarget.signals?.[0] ?? coupledTarget.signal;
  coupledSignal.sequence = 2;
  coupledTarget.expected.lastConsumedSequence = 2;
  assertFixtureRejected(coupledSequenceDrift,
    `fixture semantics accepted coupled sequence drift for ${scenarioId}`);
}

const duplicateKindsCollapsed = structuredClone(fixture);
const duplicateCase = duplicateKindsCollapsed.scenarios.find((scenario) =>
  scenario.scenarioId === "summary-provider-retry-exhausted").fault.resumeCases.find(
  (item) => item.caseId === "duplicate-and-out-of-order-no-op",
);
for (const signal of duplicateCase.signals) {
  signal.kind = "validated_configuration_activation";
}
assertFixtureRejected(duplicateKindsCollapsed,
  "fixture semantics accepted collapsed duplicate/out-of-order signal kinds");

const swappedOutputNoopKinds = structuredClone(fixture);
const outputNoops = swappedOutputNoopKinds.scenarios.find((scenario) =>
  scenario.scenarioId === "summary-provider-output-limit-exceeded").fault.resumeCases;
const outputHealth = outputNoops.find((item) =>
  item.caseId === "unchanged-provider-health-no-op");
const outputDoctor = outputNoops.find((item) =>
  item.caseId === "unchanged-doctor-retry-no-op");
[outputHealth.signals[0].kind, outputDoctor.signals[0].kind] =
  [outputDoctor.signals[0].kind, outputHealth.signals[0].kind];
assertFixtureRejected(swappedOutputNoopKinds,
  "fixture semantics accepted swapped output-limit no-op signal kinds");

const changedOutputNoopSequence = structuredClone(fixture);
changedOutputNoopSequence.scenarios.find((scenario) =>
  scenario.scenarioId === "summary-provider-output-limit-exceeded").fault.resumeCases.find(
  (item) => item.caseId === "unchanged-provider-health-no-op",
).signals[0].sequence = 2;
assertFixtureRejected(changedOutputNoopSequence,
  "fixture semantics accepted a changed output-limit no-op signal sequence");

for (const [endpointUrl, pattern, label] of [
  ["http://summary.stub.invalid/v1/chat/completions", /remote HTTPS policy/,
    "remote HTTP endpoint"],
  ["https://user@summary.stub.invalid/v1/chat/completions", /userinfo/,
    "endpoint userinfo"],
  ["https://summary.stub.invalid/v1/chat/completions?mode=test", /query or fragment/,
    "endpoint query"],
  ["https://summary.stub.invalid/v1/chat/completions#fragment", /query or fragment/,
    "endpoint fragment"],
  ["https://[::ffff:0:0]/v1/chat/completions", /hostname/,
    "IPv4-mapped unspecified endpoint"],
]) {
  const invalidEndpoint = structuredClone(fixture);
  invalidEndpoint.effectiveConfiguration.summaryProvider.endpointUrl = endpointUrl;
  assertCanonicalFixtureRejected(invalidEndpoint, pattern,
    `fixture canonical validator accepted ${label}`);
}

const credentialedLocalHttp = structuredClone(fixture);
const credentialedLocalHttpProposal = credentialedLocalHttp.scenarios.find((scenario) =>
  Object.hasOwn(scenario, "providerActivationProposal")).providerActivationProposal.proposal;
credentialedLocalHttpProposal.endpointUrl = "http://127.0.0.1:1234/v1/chat/completions";
credentialedLocalHttpProposal.credentialRef = {
  kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY",
};
assertCanonicalFixtureRejected(credentialedLocalHttp, /local HTTP endpoint must be credential-none/,
  "fixture accepted a credentialed local HTTP provider");

const localhostLocalProvider = structuredClone(fixture);
localhostLocalProvider.localDerivationManifest.summaryProvider.endpointUrl =
  "http://localhost:1234/v1/chat/completions";
assertCanonicalFixtureRejected(localhostLocalProvider, /hostname/,
  "fixture canonical validator accepted localhost as a local provider");

for (const field of ["providerKind", "headers"]) {
  const providerExtension = structuredClone(fixture);
  providerExtension.effectiveConfiguration.summaryProvider[field] = {};
  assert.notEqual(validateAgainstSchema(providerExtension, fixtureSchema, fixtureSchema).length, 0,
    `fixture schema accepted forbidden ProviderChoice field ${field}`);
}

const inlineCredential = structuredClone(fixture);
inlineCredential.effectiveConfiguration.summaryProvider.credentialRef = { kind: "inline" };
assert.notEqual(validateAgainstSchema(inlineCredential, fixtureSchema, fixtureSchema).length, 0,
  "fixture schema accepted an inline credential");

const arbitraryResourceOverride = structuredClone(fixture);
arbitraryResourceOverride.outputLimitRecoveryManifest.resourceProfile.idleFlushMs = 1;
assert.notEqual(validateAgainstSchema(
  arbitraryResourceOverride, fixtureSchema, fixtureSchema,
).length, 0, "fixture schema accepted an arbitrary resource successor override");

const mismatchedResourceProfilePair = structuredClone(fixture);
mismatchedResourceProfilePair.effectiveConfiguration.resourceProfile.version = 2;
assert.notEqual(validateAgainstSchema(
  mismatchedResourceProfilePair, fixtureSchema, fixtureSchema,
).length, 0, "fixture schema accepted profile version 2 with derivation limit 16");

const arbitraryTlsPreflightOverride = structuredClone(fixture);
arbitraryTlsPreflightOverride.outputLimitRecoveryManifest.resourceProfile
  .providerTlsPreflightTimeoutMs = 5001;
assert.notEqual(validateAgainstSchema(
  arbitraryTlsPreflightOverride, fixtureSchema, fixtureSchema,
).length, 0, "fixture schema accepted an arbitrary TLS preflight timeout");

const missingRetrievalMilestone = structuredClone(fixture);
missingRetrievalMilestone.lifecycleProfiles.bidirectional_prompt_flush =
  missingRetrievalMilestone.lifecycleProfiles.bidirectional_prompt_flush.filter(
    (name) => name !== "target_retrieval_requested",
  );
assertFixtureRejected(missingRetrievalMilestone,
  "fixture semantics accepted a selection lifecycle without retrieval");

for (const [profileId, milestone] of [["bidirectional_prompt_flush", "target_first_prompt_submitted_before_model"], ["derived_sensitivity_rejection", "validated_local_manifest_activated"]]) {
  const missingOrderedMilestone = structuredClone(fixture);
  missingOrderedMilestone.lifecycleProfiles[profileId] = missingOrderedMilestone.lifecycleProfiles[profileId].filter((name) => name !== milestone);
  assertFixtureRejected(missingOrderedMilestone, `fixture semantics accepted missing ${milestone}`);
}

const injectedForbiddenFact = structuredClone(fixture);
const injectedForbiddenScenario = injectedForbiddenFact.scenarios[0];
injectedForbiddenScenario.forbiddenFacts[0] = injectedForbiddenScenario.expectedInjectedItems[0].fact;
assertFixtureRejected(injectedForbiddenFact,
  "fixture semantics accepted an injected forbidden fact");

const inconsistentRevisionIdentity = structuredClone(fixture);
const revisionItems = inconsistentRevisionIdentity.scenarios[0].expectedInjectedItems;
revisionItems[1].lineageId = revisionItems[0].lineageId;
revisionItems[1].revisionOrdinal = revisionItems[0].revisionOrdinal + 1;
assertFixtureRejected(inconsistentRevisionIdentity,
  "fixture semantics accepted two active revisions for one lineage");

for (const identityField of ["lineageId", "memoryId", "revisionId"]) {
  const duplicateNormalIdentity = structuredClone(fixture);
  const duplicateScenario = duplicateNormalIdentity.scenarios[0];
  const retained = duplicateScenario.expectedInjectedItems[0];
  const duplicateOmission = structuredClone(duplicateScenario.expectedInjectedItems[1]);
  delete duplicateOmission.selectionReason;
  duplicateOmission.reason = "omitted_budget";
  duplicateOmission.revisionOrdinal = retained.revisionOrdinal + 1;
  duplicateOmission[identityField] = retained[identityField];
  duplicateScenario.expectedInjectedItems.splice(1, 1);
  duplicateScenario.expectedOmissions.push(duplicateOmission);
  assertFixtureRejected(duplicateNormalIdentity,
    `fixture semantics accepted duplicate normal ${identityField}`);
}

const mismatchedDuplicateRevision = structuredClone(fixture);
const duplicateScenario = mismatchedDuplicateRevision.scenarios[0];
const retainedRevision = duplicateScenario.expectedInjectedItems[0];
const duplicateRevision = duplicateScenario.expectedInjectedItems.splice(1, 1)[0];
delete duplicateRevision.selectionReason;
duplicateRevision.reason = "duplicate_revision";
for (const field of ["memoryId", "lineageId", "revisionId", "revisionOrdinal"]) {
  duplicateRevision[field] = retainedRevision[field];
}
duplicateScenario.expectedOmissions.push(duplicateRevision);
assertFixtureRejected(mismatchedDuplicateRevision,
  "fixture semantics accepted conflicting content for one duplicate revision");

for (const [metricName, invalidScenarioId] of [["warmInjectionP95Ms", "claude-to-codex"],
  ["shortColdLexicalInjectionMs", "codex-to-claude"]]) {
  const invalidMetricMode = structuredClone(fixture);
  invalidMetricMode.samplingProtocol.metrics[metricName].scenarios[0] = invalidScenarioId;
  assertFixtureRejected(invalidMetricMode,
    `fixture semantics accepted ${metricName} with the wrong reset mode`);
}

const nonInjectionWarmMetric = structuredClone(fixture);
nonInjectionWarmMetric.samplingProtocol.metrics.warmInjectionP95Ms.scenarios.push(
  "credentialless-http-activation-rejected",
);
assertFixtureRejected(nonInjectionWarmMetric,
  "fixture semantics accepted a warm injection metric without an injection lifecycle");

const emptyInjectionWarmMetric = structuredClone(fixture);
emptyInjectionWarmMetric.samplingProtocol.metrics.warmInjectionP95Ms.scenarios.push(
  "incompatible-scope-injection-rejected",
);
assertFixtureRejected(emptyInjectionWarmMetric,
  "fixture semantics accepted a warm injection metric without injected items");

const unsupportedPercentileMethod = structuredClone(fixture);
unsupportedPercentileMethod.samplingProtocol.percentileMethod = "linear_interpolation";
assertFixtureRejected(unsupportedPercentileMethod,
  "fixture semantics accepted an unsupported percentile method");

for (const [field, value] of [
  ["percentileScope", "pooled_across_scenarios"], ["clock", "wall_clock"],
]) {
  const unsupportedSamplingProtocol = structuredClone(fixture);
  unsupportedSamplingProtocol.samplingProtocol[field] = value;
  assertFixtureRejected(unsupportedSamplingProtocol,
    `fixture semantics accepted unsupported ${field}`);
}

for (const name of ["agentBlockageCount", "acceptedEventLossCount",
  "duplicateDurableMemoryCount", "secretEgressCount", "incompatibleScopeInjectionCount"]) {
  const nonzeroSafetyThreshold = structuredClone(fixture);
  nonzeroSafetyThreshold.thresholds[name] = 1;
  assert.notEqual(validateAgainstSchema(
    nonzeroSafetyThreshold, fixtureSchema, fixtureSchema,
  ).length, 0, `fixture schema accepted nonzero ${name}`);
  assertFixtureRejected(nonzeroSafetyThreshold,
    `fixture semantics accepted nonzero ${name}`);
}

console.log("Slice 1 fixture semantic regression checks passed.");
