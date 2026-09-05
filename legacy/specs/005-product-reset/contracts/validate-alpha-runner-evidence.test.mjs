import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { validateAgainstSchema } from "../../../harness/schema/validate.ts";
import { validateResourcePlateauEvidence } from "./alpha-result-resource.mjs";
import {
  networkTrustEvidenceFingerprint,
  resourcePlateauEvidenceFingerprint,
  runnerEvidenceFingerprint,
  runnerResultObservationFingerprint,
  validateRunnerEvidence,
} from "./alpha-runner-evidence.mjs";
import {
  providerEgressCommittedEventSetFingerprint,
  validateNetworkTrustEvidence,
} from "./alpha-result-security.mjs";

const contractDir = dirname(fileURLToPath(import.meta.url));
const readFixture = (name) => JSON.parse(readFileSync(join(contractDir, "../fixtures", name), "utf8"));
const fixture = readFixture("slice1-bidirectional-en-v1.json");
const result = readFixture("alpha-result-v1.failure-example.json");
const evidence = readFixture("runner-evidence/alpha-runner-evidence-v1.failure-example.json");
const suiteResults = readFixture("alpha-result-v1.suite-regression.json");
const suiteResult = suiteResults.negativeResult;
const suiteEvidence = readFixture("runner-evidence/alpha-runner-evidence-v1.suite-regression.json");
const evidenceSchema = JSON.parse(readFileSync(
  join(contractDir, "alpha-runner-evidence-v1.schema.json"), "utf8",
));

function bind(mutantEvidence, mutantResult, {
  bindNetwork = true, bindPlateau = true,
} = {}) {
  if (bindNetwork) {
    mutantResult.networkTrustEvidenceFingerprint =
      networkTrustEvidenceFingerprint(mutantEvidence.networkTrustEvidence);
  }
  if (bindPlateau) {
    mutantResult.resourcePlateauEvidenceFingerprint =
      resourcePlateauEvidenceFingerprint(mutantEvidence.resourcePlateauEvidence);
  }
  const record = mutantEvidence.scenarios.find(
    (item) => item.caseId === mutantResult.runnerEvidenceCaseId,
  );
  if (!record) {
    throw new Error(`evidence scenario is missing: ${mutantResult.runnerEvidenceCaseId}`);
  }
  record.resultObservationFingerprint = runnerResultObservationFingerprint(mutantResult);
  mutantResult.runnerEvidenceFingerprint = runnerEvidenceFingerprint(mutantEvidence);
}

function assertEvidenceRejected(mutate, pattern, label, options) {
  const mutantEvidence = structuredClone(evidence);
  const mutantResult = structuredClone(result);
  mutate(mutantEvidence, mutantResult);
  bind(mutantEvidence, mutantResult, options);
  assert.throws(() => validateRunnerEvidence(
    mutantEvidence, mutantResult, fixture, mutantEvidence.invocationId,
  ), pattern, label);
}

function assertEvidenceAccepted(mutate, label) {
  const mutantEvidence = structuredClone(evidence);
  const mutantResult = structuredClone(result);
  mutate(mutantEvidence, mutantResult);
  bind(mutantEvidence, mutantResult);
  assert.doesNotThrow(() => validateRunnerEvidence(
    mutantEvidence, mutantResult, fixture, mutantEvidence.invocationId,
  ), label);
  assert.equal(validateResourcePlateauEvidence(
    mutantEvidence.resourcePlateauEvidence, fixture,
  ), true, label);
}

function assertPlateauThresholdMiss(mutate, label) {
  const mutantEvidence = structuredClone(evidence);
  const mutantResult = structuredClone(result);
  mutate(mutantEvidence.resourcePlateauEvidence);
  bind(mutantEvidence, mutantResult);
  assert.doesNotThrow(() => validateRunnerEvidence(
    mutantEvidence, mutantResult, fixture, mutantEvidence.invocationId,
  ), label);
  assert.equal(validateResourcePlateauEvidence(
    mutantEvidence.resourcePlateauEvidence, fixture,
  ), false, label);
}

function assertSuiteRecoveryRejected(scenarioId, mutate, pattern, label) {
  const mutantEvidence = structuredClone(suiteEvidence);
  const mutantResult = structuredClone(suiteResults.positiveResults.find(
    (item) => item.scenarioId === scenarioId,
  ));
  const record = mutantEvidence.scenarios.find((item) => item.scenarioId === scenarioId);
  mutate(record, mutantResult);
  record.resultObservationFingerprint = runnerResultObservationFingerprint(mutantResult);
  mutantResult.runnerEvidenceFingerprint = runnerEvidenceFingerprint(mutantEvidence);
  assert.throws(() => validateRunnerEvidence(
    mutantEvidence, mutantResult, fixture, mutantEvidence.invocationId,
  ), pattern, label);
}

assert.deepEqual(validateAgainstSchema(evidence, evidenceSchema, evidenceSchema), [],
  "fixed runner evidence does not match its schema");
const wrongNegativeBaseEvidence = structuredClone(suiteEvidence);
const wrongNegativeBaseResult = structuredClone(suiteResults.positiveResults[0]);
wrongNegativeBaseEvidence.scenarios.find((item) =>
  item.caseId === fixture.beforeModelNegativeFixture.caseId
).scenarioId = "codex-to-claude";
wrongNegativeBaseResult.runnerEvidenceFingerprint =
  runnerEvidenceFingerprint(wrongNegativeBaseEvidence);
assert.throws(() => validateRunnerEvidence(
  wrongNegativeBaseEvidence, wrongNegativeBaseResult, fixture,
  wrongNegativeBaseEvidence.invocationId,
), /late-injection negative does not match its fixed base scenario/,
  "runner evidence accepted a late-injection negative for another scenario");
const missingAttemptedRecoveryResult = structuredClone(suiteResults.positiveResults.find(
  (item) => item.scenarioId === "summary-provider-retry-exhausted",
));
const missingAttemptedRecoveryEvidence = structuredClone(suiteEvidence);
missingAttemptedRecoveryEvidence.scenarios.find((item) =>
  item.scenarioId === missingAttemptedRecoveryResult.scenarioId
).recoveryProviderEgressEvidence = [];
missingAttemptedRecoveryResult.runnerEvidenceFingerprint =
  runnerEvidenceFingerprint(missingAttemptedRecoveryEvidence);
assert.throws(() => validateRunnerEvidence(
  missingAttemptedRecoveryEvidence, missingAttemptedRecoveryResult, fixture,
  missingAttemptedRecoveryEvidence.invocationId,
), /recovery provider egress/,
  "runner evidence accepted missing attempted recovery-provider observations");
const contradictedRecoveryResult = structuredClone(suiteResults.positiveResults.find(
  (item) => item.scenarioId === "summary-provider-retry-exhausted",
));
contradictedRecoveryResult.retryEvidence.cases.find((item) =>
  item.caseId === "validated-configuration-activation"
).providerAttempted = false;
const contradictedRecoveryEvidence = structuredClone(suiteEvidence);
contradictedRecoveryEvidence.scenarios.find((item) =>
  item.scenarioId === contradictedRecoveryResult.scenarioId
).resultObservationFingerprint = runnerResultObservationFingerprint(contradictedRecoveryResult);
contradictedRecoveryResult.runnerEvidenceFingerprint =
  runnerEvidenceFingerprint(contradictedRecoveryEvidence);
assert.throws(() => validateRunnerEvidence(
  contradictedRecoveryEvidence, contradictedRecoveryResult, fixture,
  contradictedRecoveryEvidence.invocationId,
), /recovery provider egress.*result/,
  "runner evidence accepted a candidate-fabricated no-op recovery result");
assertSuiteRecoveryRejected("summary-provider-output-limit-exceeded", (record) => {
  record.recoveryProviderEgressEvidence = record.recoveryProviderEgressEvidence.filter(
    (item) => item.caseId !== "unchanged-provider-health-no-op",
  );
}, /recovery provider egress.*incomplete/,
  "runner evidence accepted a missing no-op recovery observation");
assertSuiteRecoveryRejected("summary-provider-output-limit-exceeded", (record) => {
  record.recoveryProviderEgressEvidence.find((item) =>
    item.caseId === "validated-larger-limit-activation"
  ).effectiveManifestFingerprint = fixture.effectiveConfiguration.configurationFingerprint;
}, /recovery provider egress.*manifest\/provider/,
  "runner evidence accepted a base manifest for output-limit recovery");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  record.recoveryProviderEgressEvidence.find((item) =>
    item.caseId === "validated-configuration-activation"
  ).evidence.providerFingerprint =
    fixture.effectiveConfiguration.summaryProvider.providerFingerprint;
}, /active provider/,
  "runner evidence accepted the base provider for repaired recovery");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  record.recoveryProviderEgressEvidence[0].evidence.receiptId =
    record.providerEgressEvidence.receiptId;
}, /receipt identities are reused/,
  "runner evidence reused an initial receipt for recovery");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  record.recoveryProviderEgressEvidence[0].evidence.processTreeRootId =
    "stale-recovery-process-tree";
}, /process tree/,
  "runner evidence accepted a recovery receipt from another process tree");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  record.recoveryProviderEgressEvidence[0].evidence.runnerInvocationId = "stale-invocation";
}, /invocation/,
  "runner evidence accepted a recovery receipt from another invocation");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  const [first, second] = record.recoveryProviderEgressEvidence;
  second.processTreeRootId = first.processTreeRootId;
  second.evidence.processTreeRootId = first.processTreeRootId;
}, /process-tree identities are reused/,
  "runner evidence reused one process tree across recovery cases");
assertSuiteRecoveryRejected("summary-provider-output-limit-exceeded", (record) => {
  const first = record.recoveryProviderEgressEvidence.find((item) =>
    item.caseId === "unchanged-provider-health-no-op");
  const second = record.recoveryProviderEgressEvidence.find((item) =>
    item.caseId === "unchanged-doctor-retry-no-op");
  const secondReceiptId = second.evidence.receiptId;
  const secondProcessTreeRootId = second.evidence.processTreeRootId;
  second.evidence = structuredClone(first.evidence);
  second.evidence.receiptId = secondReceiptId;
  second.evidence.processTreeRootId = secondProcessTreeRootId;
}, /observation case/,
  "runner evidence reused one no-op receipt across recovery cases");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  const evidence = record.recoveryProviderEgressEvidence[0].evidence;
  evidence.authorization.observedAtMonotonicMs = evidence.candidateStartedMonotonicMs;
}, /runner-owned authorization/,
  "runner evidence accepted recovery egress before authorization");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  record.recoveryProviderEgressEvidence[0].evidence.credentialBytesSent += 1;
}, /wire aggregates/,
  "runner evidence accepted recovery credential-byte drift");
assertSuiteRecoveryRejected("summary-provider-retry-exhausted", (record) => {
  record.recoveryProviderEgressEvidence[0].evidence
    .sourcePayloadBytesBySensitivity.eligible += 1;
}, /sensitivity-byte evidence/,
  "runner evidence accepted recovery sensitivity-byte drift");
assertSuiteRecoveryRejected("summary-provider-output-limit-exceeded", (record) => {
  record.recoveryProviderEgressEvidence.find((item) =>
    item.caseId === "unchanged-provider-health-no-op"
  ).evidence.nonLoopbackSocketAttemptCount = 1;
}, /wire aggregates/,
  "runner evidence accepted hidden egress in a recovery no-op case");
const missingProviderEgress = structuredClone(evidence);
delete missingProviderEgress.scenarios[0].providerEgressEvidence;
assert.notEqual(validateAgainstSchema(
  missingProviderEgress, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing provider egress evidence");
for (const field of ["restrictedPayloadBytesSent", "forbiddenSentinelObservationCount"]) {
  const missingRunnerAggregate = structuredClone(evidence);
  delete missingRunnerAggregate.scenarios[0].providerEgressEvidence[field];
  assert.notEqual(validateAgainstSchema(
    missingRunnerAggregate, evidenceSchema, evidenceSchema,
  ).length, 0, `runner evidence schema accepted missing ${field}`);
}
assertEvidenceRejected((mutant) => {
  mutant.scenarios[0].providerEgressEvidence.runnerInvocationId = "stale-invocation";
}, /invocation/,
  "runner evidence accepted an initial egress receipt from another invocation");
assertEvidenceRejected((mutant) => {
  mutant.scenarios[0].providerEgressEvidence.observationCaseId = "other-case";
}, /observation case/,
  "runner evidence accepted an initial egress receipt from another case");
for (const field of ["observationCaseId", "runnerInvocationId", "processTreeRootId"]) {
  const missingRunBinding = structuredClone(evidence);
  delete missingRunBinding.scenarios[0].providerEgressEvidence[field];
  assert.notEqual(validateAgainstSchema(
    missingRunBinding, evidenceSchema, evidenceSchema,
  ).length, 0, `runner evidence schema accepted missing initial ${field}`);
}
for (const [target, field] of [
  ["wrapper", "processTreeRootId"],
  ["receipt", "observationCaseId"],
  ["receipt", "runnerInvocationId"],
  ["receipt", "processTreeRootId"],
]) {
  const missingRecoveryBinding = structuredClone(suiteEvidence);
  const wrapper = missingRecoveryBinding.scenarios.find((item) =>
    item.scenarioId === "summary-provider-retry-exhausted"
  ).recoveryProviderEgressEvidence[0];
  delete (target === "wrapper" ? wrapper : wrapper.evidence)[field];
  assert.notEqual(validateAgainstSchema(
    missingRecoveryBinding, evidenceSchema, evidenceSchema,
  ).length, 0, `runner evidence schema accepted missing recovery ${target} ${field}`);
}
const missingRecoveryProviderEgress = structuredClone(evidence);
delete missingRecoveryProviderEgress.scenarios[0].recoveryProviderEgressEvidence;
assert.notEqual(validateAgainstSchema(
  missingRecoveryProviderEgress, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing recovery provider egress evidence");
const missingCommittedEventIds = structuredClone(evidence);
delete missingCommittedEventIds.scenarios[0].providerEgressEvidence.authorization.committedEventIds;
assert.notEqual(validateAgainstSchema(
  missingCommittedEventIds, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing committed event identities");
for (const name of ["networkTrustEvidence", "resourcePlateauEvidence"]) {
  const missing = structuredClone(evidence);
  delete missing[name];
  assert.notEqual(validateAgainstSchema(missing, evidenceSchema, evidenceSchema).length, 0,
    `runner evidence schema accepted missing ${name}`);
}
const missingPublicCa = structuredClone(evidence);
delete missingPublicCa.networkTrustEvidence.publicCaSha256;
assert.notEqual(validateAgainstSchema(
  missingPublicCa, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing public CA fingerprint");
const missingNetworkInvocation = structuredClone(evidence);
delete missingNetworkInvocation.networkTrustEvidence.runnerInvocationId;
assert.notEqual(validateAgainstSchema(
  missingNetworkInvocation, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing network invocation identity");
const missingTlsReceipts = structuredClone(evidence);
delete missingTlsReceipts.networkTrustEvidence.tlsPreflightReceipts;
assert.notEqual(validateAgainstSchema(
  missingTlsReceipts, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing TLS preflight receipts");
for (const field of ["trustAnchorSha256", "peerCertificateSha256"]) {
  const missingTlsFingerprint = structuredClone(evidence);
  delete missingTlsFingerprint.networkTrustEvidence.tlsPreflightReceipts[0][field];
  assert.notEqual(validateAgainstSchema(
    missingTlsFingerprint, evidenceSchema, evidenceSchema,
  ).length, 0, `runner evidence schema accepted missing TLS ${field}`);
}
const missingTlsInvocation = structuredClone(evidence);
delete missingTlsInvocation.networkTrustEvidence.tlsPreflightReceipts[0].runnerInvocationId;
assert.notEqual(validateAgainstSchema(
  missingTlsInvocation, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing TLS receipt invocation identity");

for (const field of ["candidateId", "artifactFingerprint", "environmentFingerprint",
  "runnerInvocationId", "processTreeRootId"]) {
  const missingPlateauIdentity = structuredClone(evidence);
  delete missingPlateauIdentity.resourcePlateauEvidence[field];
  assert.notEqual(validateAgainstSchema(
    missingPlateauIdentity, evidenceSchema, evidenceSchema,
  ).length, 0, `runner evidence schema accepted missing plateau ${field}`);
}
for (const field of ["drainReceiptId", "checkpointReceiptId"]) {
  const missingReceipt = structuredClone(evidence);
  delete missingReceipt.resourcePlateauEvidence.windows[0][field];
  assert.notEqual(validateAgainstSchema(
    missingReceipt, evidenceSchema, evidenceSchema,
  ).length, 0, `runner evidence schema accepted missing ${field}`);
}
const missingWorkloadReceipt = structuredClone(evidence);
delete missingWorkloadReceipt.resourcePlateauEvidence.windows[0].workloadReceiptId;
assert.notEqual(validateAgainstSchema(
  missingWorkloadReceipt, evidenceSchema, evidenceSchema,
).length, 0, "runner evidence schema accepted missing workload receipt");
for (const field of ["workloadStartedMonotonicMs", "workloadReceiptMonotonicMs",
  "drainReceiptMonotonicMs", "checkpointReceiptMonotonicMs", "resourceSampleMonotonicMs"]) {
  const missingTimestamp = structuredClone(evidence);
  delete missingTimestamp.resourcePlateauEvidence.windows[0][field];
  assert.notEqual(validateAgainstSchema(
    missingTimestamp, evidenceSchema, evidenceSchema,
  ).length, 0, `runner evidence schema accepted missing ${field}`);
}
const orphanThresholdEvidence = structuredClone(evidence);
orphanThresholdEvidence.resourcePlateauEvidence.orphanProductProcessCount = 1;
assert.deepEqual(validateAgainstSchema(
  orphanThresholdEvidence, evidenceSchema, evidenceSchema,
), [], "runner evidence schema rejected an inspectable orphan-process threshold miss");
for (const [field, value, label] of [
  ["candidateId", "stale-plateau-candidate", "candidate"],
  ["artifactFingerprint", `sha256:${"0".repeat(64)}`, "artifact"],
  ["environmentFingerprint", `sha256:${"0".repeat(64)}`, "environment"],
  ["runnerInvocationId", "stale-plateau-invocation", "invocation"],
]) assertEvidenceRejected((mutant) => {
  mutant.resourcePlateauEvidence[field] = value;
}, /resource plateau.*identity/,
  `runner evidence accepted a plateau from another ${label}`);
assertEvidenceRejected((mutantEvidence, mutantResult) => {
  mutantEvidence.resourcePlateauEvidence = null;
  mutantResult.resourcePlateauEvidenceFingerprint = null;
}, /executed runner evidence is missing its resource plateau/,
  "executed runner evidence omitted the resource plateau", { bindPlateau: false });
assertEvidenceRejected((mutant) => {
  mutant.resourcePlateauEvidence.processTreeRootId = mutant.scenarios[0].processTreeRootId;
}, /process-tree identities are reused/,
  "runner evidence reused an initial process tree for the resource plateau");

assertEvidenceRejected((mutant) => {
  const changedCa = `sha256:${"0".repeat(64)}`;
  mutant.networkTrustEvidence.publicCaSha256 = changedCa;
  for (const receipt of mutant.networkTrustEvidence.tlsPreflightReceipts) {
    receipt.trustAnchorSha256 = changedCa;
  }
}, /network trust evidence fingerprint/, "modified public CA fingerprint", {
  bindNetwork: false,
});
for (const [field, value, pattern] of [
  ["chainValidation", false, /chain validation/],
  ["hostnameValidation", false, /hostname validation/],
  ["privateKeyCommitted", true, /private key/],
]) {
  assertEvidenceRejected((mutant) => {
    mutant.networkTrustEvidence[field] = value;
  }, pattern, `invalid network trust ${field}`);
}
assertEvidenceRejected((mutant) => {
  mutant.networkTrustEvidence.baseHostname = "other.invalid";
}, /fixed hostnames/, "network trust hostname drift");
assertEvidenceRejected((mutant) => {
  mutant.networkTrustEvidence.localHostname = "127.0.0.2";
}, /fixed hostnames/, "network trust local hostname drift");
assertEvidenceRejected((mutant) => {
  mutant.networkTrustEvidence.runnerInvocationId = "stale-network-invocation";
  for (const receipt of mutant.networkTrustEvidence.tlsPreflightReceipts) {
    receipt.runnerInvocationId = "stale-network-invocation";
  }
}, /network trust.*invocation/,
  "runner evidence accepted TLS trust from another invocation");
assertEvidenceRejected((mutant) => {
  mutant.networkTrustEvidence.tlsPreflightReceipts[0].runnerInvocationId = "stale-invocation";
}, /network trust.*invocation/,
  "runner evidence accepted a TLS receipt from another invocation");
assertEvidenceRejected((_mutant, mutantResult) => {
  mutantResult.networkTrustEvidenceFingerprint = `sha256:${"0".repeat(64)}`;
}, /network trust evidence fingerprint/, "stale network trust evidence fingerprint", {
  bindNetwork: false,
});

const ipv6Fixture = structuredClone(fixture);
ipv6Fixture.localDerivationManifest.summaryProvider.endpointUrl =
  "https://[::1]:1234/v1/chat/completions";
const ipv6Network = structuredClone(evidence.networkTrustEvidence);
ipv6Network.localHostname = "[::1]";
for (const receipt of ipv6Network.tlsPreflightReceipts.filter(
  (item) => item.hostname === "127.0.0.1",
)) {
  receipt.hostname = "[::1]";
  receipt.sni = null;
}
assert.doesNotThrow(() => validateNetworkTrustEvidence(ipv6Network, ipv6Fixture),
  "network trust evidence rejected null SNI for an IPv6 literal");
const ipv6SchemaEvidence = structuredClone(evidence);
ipv6SchemaEvidence.networkTrustEvidence = ipv6Network;
assert.deepEqual(validateAgainstSchema(
  ipv6SchemaEvidence, evidenceSchema, evidenceSchema,
), [], "runner evidence schema rejected the supported IPv6 loopback literal");

const tlsReceiptMutations = [
  [(network) => { network.tlsPreflightReceipts.pop(); }, /exactly six/,
    "missing TLS preflight receipt"],
  [(network) => { network.tlsPreflightReceipts[1].receiptId =
    network.tlsPreflightReceipts[0].receiptId; }, /receipt.*unique/,
    "reused TLS preflight receipt ID"],
  [(network) => { network.tlsPreflightReceipts[0].receiptId = "../preflight"; },
    /path-free opaque/, "path-like TLS preflight receipt ID"],
  [(network) => { network.tlsPreflightReceipts[0].phase = "daemon_start"; },
    /pair set/, "duplicate TLS preflight phase/hostname pair"],
  [(network) => { network.tlsPreflightReceipts[0].hostname = "other.invalid"; },
    /pair set/, "unknown TLS preflight hostname"],
  [(network) => { network.tlsPreflightReceipts[0].sni = "other.invalid"; },
    /SNI/, "TLS preflight SNI mismatch"],
  [(network) => { network.tlsPreflightReceipts[0].port = 80; },
    /port/, "TLS preflight port mismatch"],
  [(network) => { network.tlsPreflightReceipts[0].timeoutMs = 5001; },
    /timeout/, "TLS preflight timeout mismatch"],
  [(network) => { network.tlsPreflightReceipts[0].endMonotonicMs =
    network.tlsPreflightReceipts[0].startMonotonicMs + 5001; }, /duration/,
    "TLS preflight over-time duration"],
  [(network) => { network.tlsPreflightReceipts[0].endMonotonicMs =
    network.tlsPreflightReceipts[0].startMonotonicMs - 1; }, /monotonic/,
    "TLS preflight reversed time"],
  [(network) => { network.tlsPreflightReceipts[0].result = "failed"; },
    /verified/, "unverified TLS preflight result"],
  [(network) => { network.tlsPreflightReceipts[0].chainValidation = false; },
    /chain validation/, "TLS preflight chain validation disabled"],
  [(network) => { network.tlsPreflightReceipts[0].hostnameValidation = false; },
    /hostname validation/, "TLS preflight hostname validation disabled"],
  [(network) => { network.tlsPreflightReceipts[0].credentialBytesSent = 1; },
    /credential bytes/, "TLS preflight credential bytes"],
  [(network) => { network.tlsPreflightReceipts[0].payloadBytesSent = 1; },
    /payload bytes/, "TLS preflight payload bytes"],
  [(network) => { network.tlsPreflightReceipts[0].httpRequestCount = 1; },
    /HTTP request/, "TLS preflight HTTP request"],
  [(network) => { network.tlsPreflightReceipts[0].trustAnchorSha256 =
    `sha256:${"0".repeat(64)}`; }, /trust anchor/, "TLS preflight trust anchor mismatch"],
  [(network) => { network.tlsPreflightReceipts[0].peerCertificateSha256 = "invalid"; },
    /peer certificate/, "TLS preflight malformed peer certificate fingerprint"],
  [(network) => { network.tlsPreflightReceipts[0].peerCertificateSha256 =
    network.publicCaSha256; }, /peer certificate/, "TLS preflight peer equals trust anchor"],
  [(network) => { network.tlsPreflightReceipts[1].peerCertificateSha256 =
    `sha256:${"0".repeat(64)}`; }, /peer certificate.*drift/,
    "TLS preflight peer certificate drift between setup and daemon start"],
  [(network) => {
    const [setup, daemon] = network.tlsPreflightReceipts;
    [setup.startMonotonicMs, setup.endMonotonicMs,
      daemon.startMonotonicMs, daemon.endMonotonicMs] = [200, 300, 0, 100];
  }, /setup.*before.*daemon|lifecycle phase/,
  "TLS setup preflight did not precede daemon start"],
];
for (const [mutate, pattern, label] of tlsReceiptMutations) {
  assertEvidenceRejected((mutant) => mutate(mutant.networkTrustEvidence), pattern, label);
}
assertEvidenceRejected((mutant) => {
  mutant.networkTrustEvidence.tlsPreflightReceipts[0].receiptId =
    "tls-preflight:base:setup-activation:changed";
}, /network trust evidence fingerprint/, "modified TLS preflight receipt fingerprint", {
  bindNetwork: false,
});

assertEvidenceRejected((mutant) => {
  mutant.scenarios[0].providerEgressEvidence.preAuthorizationProviderAttemptCount = 1;
}, /wire aggregates/, "provider attempted egress before runner authorization");
assertEvidenceRejected((mutant) => {
  const egress = mutant.scenarios[0].providerEgressEvidence;
  egress.firstProviderRequestStartedMonotonicMs = egress.authorization.observedAtMonotonicMs;
}, /strictly after runner-owned authorization/, "provider request at authorization boundary");
assertEvidenceRejected((mutant) => {
  mutant.scenarios[0].providerEgressEvidence.authorization.committedEventSetFingerprint =
    `sha256:${"0".repeat(64)}`;
}, /provider authorization event set does not match committed events/,
  "provider authorization bound wrong event set");
assertEvidenceRejected((mutant) => {
  const authorization = mutant.scenarios[0].providerEgressEvidence.authorization;
  authorization.committedEventIds.push(authorization.committedEventIds[0]);
}, /authorization event identities/, "provider authorization repeated one event identity");
assertEvidenceRejected((mutant) => {
  mutant.scenarios[0].providerEgressEvidence.sourcePayloadBytesBySensitivity.secret = 1;
}, /sensitivity-byte/, "provider egress included secret source bytes");
assertEvidenceRejected((mutantEvidence, mutantResult) => {
  const observed = mutantEvidence.scenarios[0].providerEgressEvidence;
  const sourceBytes = Object.values(observed.sourcePayloadBytesBySensitivity)
    .reduce((sum, value) => sum + value, 0);
  observed.payloadBytesSent = sourceBytes - 1;
  mutantResult.securityEvidence.payloadBytesSent = sourceBytes - 1;
}, /source sensitivity bytes exceed the provider payload/,
  "provider egress counted more source bytes than payload bytes");
assertEvidenceRejected((mutantEvidence, mutantResult) => {
  mutantEvidence.scenarios[0].providerEgressEvidence.restrictedPayloadBytesSent = 1;
  mutantResult.securityEvidence.restrictedPayloadBytesSent = 1;
}, /runner observed restricted provider payload/,
  "provider egress receipt accepted restricted payload bytes");
assertEvidenceRejected((mutantEvidence, mutantResult) => {
  mutantEvidence.scenarios[0].providerEgressEvidence.forbiddenSentinelObservationCount = 1;
  mutantResult.securityEvidence.forbiddenSentinelObservationCount = 1;
}, /runner observed a forbidden sentinel/,
  "provider egress receipt accepted a forbidden sentinel");

const wrongNegativeProjection = structuredClone(suiteEvidence);
wrongNegativeProjection.scenarios.find((item) =>
  item.caseId === fixture.beforeModelNegativeFixture.caseId)
  .providerEgressEvidence.sourceCaseId = "codex-to-claude";
const wrongNegativeProjectionResult = structuredClone(suiteResult);
wrongNegativeProjectionResult.runnerEvidenceFingerprint =
  runnerEvidenceFingerprint(wrongNegativeProjection);
assert.throws(() => validateRunnerEvidence(
  wrongNegativeProjection,
  wrongNegativeProjectionResult,
  fixture,
  wrongNegativeProjection.invocationId,
), /late-injection negative does not project its fixed base egress receipt/);

const nonPrefixResult = structuredClone(suiteResults.positiveResults.find((item) =>
  item.runnerEvidenceCaseId === "claude-to-codex"));
const nonPrefixEvidence = structuredClone(suiteEvidence);
const nonPrefixRecord = nonPrefixEvidence.scenarios.find((item) =>
  item.caseId === nonPrefixResult.runnerEvidenceCaseId);
const nonPrefixScenario = fixture.scenarios.find((item) =>
  item.scenarioId === nonPrefixResult.scenarioId);
const committedEvent = nonPrefixScenario.events[1];
nonPrefixResult.counts.committed = 1;
nonPrefixRecord.providerEgressEvidence.authorization.committedEventCount = 1;
nonPrefixRecord.providerEgressEvidence.authorization.committedEventIds = [committedEvent.eventId];
nonPrefixRecord.providerEgressEvidence.authorization.committedEventSetFingerprint =
  providerEgressCommittedEventSetFingerprint(
    [committedEvent], nonPrefixScenario.sourceRepositoryScope,
  );
nonPrefixRecord.providerEgressEvidence.sourcePayloadBytesBySensitivity = {
  eligible: Buffer.byteLength(committedEvent.redactedPayload, "utf8"),
  localOnly: 0,
  private: 0,
  secret: 0,
};
nonPrefixRecord.resultObservationFingerprint = runnerResultObservationFingerprint(nonPrefixResult);
nonPrefixResult.runnerEvidenceFingerprint = runnerEvidenceFingerprint(nonPrefixEvidence);
assert.doesNotThrow(() => validateRunnerEvidence(
  nonPrefixEvidence, nonPrefixResult, fixture, nonPrefixEvidence.invocationId,
), "runner evidence rejected an explicit non-prefix committed event set");

const plateauMutations = [
  [(plateau) => { plateau.windows.pop(); }, /exactly 12/, "missing plateau window"],
  [(plateau) => { plateau.windows[0].workloadReceiptMonotonicMs =
    plateau.windows[0].workloadStartedMonotonicMs; }, /workload.*order/,
    "workload receipt did not follow workload start"],
  [(plateau) => { plateau.windows[0].drainReceiptMonotonicMs =
    plateau.windows[0].workloadReceiptMonotonicMs; }, /workload.*order/,
    "drain receipt did not follow workload receipt"],
  [(plateau) => { plateau.windows[0].checkpointReceiptMonotonicMs =
    plateau.windows[0].drainReceiptMonotonicMs; }, /workload.*order/,
    "checkpoint receipt did not follow drain receipt"],
  [(plateau) => { plateau.windows[0].resourceSampleMonotonicMs =
    plateau.windows[0].checkpointReceiptMonotonicMs; }, /workload.*order/,
    "resource sample did not follow checkpoint receipt"],
  [(plateau) => { plateau.windows[1].workloadStartedMonotonicMs =
    plateau.windows[0].resourceSampleMonotonicMs; }, /workload.*order/,
    "resource plateau windows overlap"],
  [(plateau) => { [plateau.windows[0], plateau.windows[1]] =
    [plateau.windows[1], plateau.windows[0]]; }, /ordered/, "unordered plateau windows"],
  [(plateau) => { plateau.windows[1].drainReceiptId = plateau.windows[0].drainReceiptId; },
    /receipt.*unique/, "reused plateau drain receipt"],
  [(plateau) => { plateau.windows[0].checkpointReceiptId = "../checkpoint"; },
    /path-free opaque/, "non-opaque plateau checkpoint receipt"],
  [(plateau) => { plateau.windows[1].workloadReceiptId =
    plateau.windows[0].workloadReceiptId; }, /workload receipt.*unique/,
    "reused plateau workload receipt"],
  [(plateau) => { plateau.windows[0].workloadReceiptId = "../workload"; },
    /workload receipt.*path-free opaque/, "path-like plateau workload receipt"],
  [(plateau) => { plateau.windows[0].duplicateDeliveryAttemptCount = 0; },
    /duplicate delivery/, "missing plateau duplicate delivery attempt"],
  [(plateau) => { plateau.windows[7].duplicateDeliveryAttemptCount = 2; },
    /duplicate delivery/, "non-identical plateau duplicate delivery count"],
  [(plateau) => { plateau.windows[0].noOpOutcome = "completed"; },
    /duplicate-no-op outcome/, "wrong plateau no-op outcome"],
  [(plateau) => { plateau.windows[0].durableMemoryDelta = 1; },
    /durable memory delta/, "nonzero plateau durable memory delta"],
  [(plateau) => { plateau.windows[0].processingJobDelta = 1; },
    /processing job delta/, "nonzero plateau processing job delta"],
  [(plateau) => { plateau.windows[0].workloadReceiptId =
    plateau.windows[0].drainReceiptId; }, /globally unique/,
    "cross-kind plateau receipt reuse"],
];
for (const [mutate, pattern, label] of plateauMutations) {
  assertEvidenceRejected((mutant) => mutate(mutant.resourcePlateauEvidence), pattern, label);
}

for (const [mutate, label] of [
  [(plateau) => { plateau.windows[11].rssMiB = plateau.windows[7].rssMiB + 17; },
    "final plateau RSS span"],
  [(plateau) => { plateau.windows[11].storageBytes =
    plateau.windows[7].storageBytes + 65537; }, "final plateau storage span"],
  [(plateau) => { plateau.windows[8].drainedQueueDepth = 1; },
    "nonzero final plateau queue"],
  [(plateau) => { plateau.windows[9].selectedItemCount += 1; },
    "nonconstant final plateau items"],
  [(plateau) => { plateau.windows[9].injectedTokenCount += 1; },
    "nonconstant final plateau tokens"],
  [(plateau) => { plateau.windows[4].maxProcessingConcurrency = 3; },
    "over-limit plateau concurrency"],
  [(plateau) => { plateau.windows[3].processCount =
    fixture.thresholds.maxSteadyProductProcessCount + 1; },
    "over-limit measured process count"],
  [(plateau) => { plateau.windows[3].drainedQueueDepth =
    fixture.thresholds.maxPendingQueueDepth + 1; },
    "over-limit measured queue depth"],
  [(plateau) => { plateau.windows[3].selectedItemCount =
    fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.maxSelectedItems + 1; },
    "over-limit measured selected items"],
  [(plateau) => { plateau.windows[3].injectedTokenCount =
    fixture.effectiveConfiguration.resourceProfile.injectionEnvelope.maxInjectedTokens + 1; },
    "over-limit measured injected tokens"],
  [(plateau) => { plateau.windows[9].processCount += 1; },
    "nonconstant final plateau process count"],
  [(plateau) => { plateau.orphanProductProcessCount = 1; }, "plateau orphan process"],
  [(plateau) => {
    plateau.windows[3].rssMiB = plateau.windows[2].rssMiB + 33;
  }, "measured RSS growth from first window"],
  [(plateau) => {
    plateau.windows[3].storageBytes = plateau.windows[2].storageBytes + 1048577;
  }, "measured storage growth from first window"],
]) assertPlateauThresholdMiss(mutate, label);

assertEvidenceAccepted((mutant) => {
  mutant.resourcePlateauEvidence.windows[2].rssMiB = 200;
}, "measured RSS decrease after a high first window");
assertEvidenceAccepted((mutant) => {
  mutant.resourcePlateauEvidence.windows[2].storageBytes = 2000000;
}, "measured storage decrease after a high first window");
assertEvidenceRejected((_mutant, mutantResult) => {
  mutantResult.resourcePlateauEvidenceFingerprint = `sha256:${"0".repeat(64)}`;
}, /resource plateau evidence fingerprint/, "stale plateau evidence fingerprint", {
  bindPlateau: false,
});
assertEvidenceRejected((mutant) => {
  mutant.resourcePlateauEvidence.windows[0].drainReceiptId =
    "plateau-window-1:changed-drain-receipt";
}, /resource plateau evidence fingerprint/, "modified plateau receipt fingerprint", {
  bindPlateau: false,
});

assertEvidenceRejected((mutant) => {
  mutant.scenarios[0].runPreparations[0].observedProductProcessCount =
    fixture.thresholds.maxSteadyProductProcessCount + 1;
},
  /warm runner preparation did not prove retained ready state/);

console.log("Alpha runner evidence regression checks passed.");
