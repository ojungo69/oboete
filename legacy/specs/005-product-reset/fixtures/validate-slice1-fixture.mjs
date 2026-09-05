import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, URL } from "node:url";

import { canonicalizeJson, readIJsonFile } from "../../../harness/schema/jcs.ts";
import { validateAgainstSchema } from "../../../harness/schema/validate.ts";
import { lineageDigest } from "../contracts/alpha-result-lineage.mjs";

const fixtureDir = dirname(fileURLToPath(import.meta.url));
const defaultFixturePath = join(fixtureDir, "slice1-bidirectional-en-v1.json");
const schemaPath = join(fixtureDir, "slice1-bidirectional-en-v1.schema.json");
const semanticPath = join(fixtureDir, "slice1-bidirectional-en-v1.semantic.jq");
const validatorPath = fileURLToPath(import.meta.url);
const resultSchemaPath = join(fixtureDir, "../contracts/alpha-result-v1.schema.json");
const resultSemanticPath = join(fixtureDir, "../contracts/alpha-result-v1.semantic.jq");
const runnerEvidenceSchemaPath = join(fixtureDir, "../contracts/alpha-runner-evidence-v1.schema.json");
const runnerEvidenceValidatorPath = join(fixtureDir, "../contracts/alpha-runner-evidence.mjs");
const resultArtifactValidatorPath = join(fixtureDir, "../contracts/alpha-result-artifact.mjs");
const resultAtomicityValidatorPath = join(fixtureDir, "../contracts/alpha-result-atomicity.mjs");
const resultInputValidatorPath = join(fixtureDir, "../contracts/alpha-result-input.mjs");
const resultLineageValidatorPath = join(fixtureDir, "../contracts/alpha-result-lineage.mjs");
const resultLatencyValidatorPath = join(fixtureDir, "../contracts/alpha-result-latency.mjs");
const resultRetryValidatorPath = join(fixtureDir, "../contracts/alpha-result-retry.mjs");
const resultResourceValidatorPath = join(fixtureDir, "../contracts/alpha-result-resource.mjs");
const resultSecurityValidatorPath = join(fixtureDir, "../contracts/alpha-result-security.mjs");
const resultRenderValidatorPath = join(fixtureDir, "../contracts/alpha-result-render.mjs");
const resultSelectionValidatorPath = join(fixtureDir, "../contracts/alpha-result-selection.mjs");
const resultValidatorPath = join(fixtureDir, "../contracts/validate-alpha-result.mjs");
const sharedJcsRuntimePath = join(fixtureDir, "../../../harness/schema/jcs.ts");
const sharedSchemaRuntimePath = join(fixtureDir, "../../../harness/schema/validate.ts");
const normalizeText = (value) => value.replace(/\r\n?/g, "\n");
const args = process.argv.slice(2);

if (args.length === 1 && args[0] === "--help") {
  console.log("Usage: node --experimental-strip-types validate-slice1-fixture.mjs [--fixture PATH]");
  process.exit(0);
}
if (args.length !== 0 && (args.length !== 2 || args[0] !== "--fixture" || !args[1])) {
  throw new Error("invalid arguments; use --help for usage");
}

const fixturePath = args.length === 0 ? defaultFixturePath : resolve(args[1]);

const fixture = readIJsonFile(fixturePath);
const schema = readIJsonFile(schemaPath);
const resultSchema = readIJsonFile(resultSchemaPath);
const runnerEvidenceSchema = readIJsonFile(runnerEvidenceSchemaPath);
const issues = validateAgainstSchema(fixture, schema, schema);

if (issues.length > 0) {
  console.error(JSON.stringify(issues, null, 2));
  process.exit(1);
}

const { contractFingerprint: _contractFingerprint, ...contract } = fixture;
const fixtureContractDomain = "free-mem:slice1-fixture-contract:v1\0";
const expectedContractFingerprintRecord =
  "fixture-contract-fingerprint=sha256:b36bcd7d3b2d98b629d797844beec52c57e67f5152a9297fdbaff8a58832f2e3";
const expectedContractFingerprint = expectedContractFingerprintRecord.replace(
  "fixture-contract-fingerprint=",
  "",
);
const fingerprint = (domain, value) => `sha256:${createHash("sha256")
  .update(domain).update(canonicalizeJson(value)).digest("hex")}`;
const actualContractFingerprint = fingerprint(fixtureContractDomain, {
    fixture: contract,
    schema,
    semanticValidator: normalizeText(readFileSync(semanticPath, "utf8")),
    canonicalValidator: normalizeText(readFileSync(validatorPath, "utf8")).replace(
      /fixture-contract-fingerprint=sha256:[0-9a-f]{64}/,
      "fixture-contract-fingerprint=<normalized>",
    ),
    resultSchema,
    resultSemanticValidator: normalizeText(readFileSync(resultSemanticPath, "utf8")),
    runnerEvidenceSchema,
    runnerEvidenceValidator: normalizeText(readFileSync(runnerEvidenceValidatorPath, "utf8")),
    resultArtifactValidator: normalizeText(readFileSync(resultArtifactValidatorPath, "utf8")),
    resultAtomicityValidator: normalizeText(readFileSync(resultAtomicityValidatorPath, "utf8")),
    resultInputValidator: normalizeText(readFileSync(resultInputValidatorPath, "utf8")),
    resultLineageValidator: normalizeText(readFileSync(resultLineageValidatorPath, "utf8")),
    resultLatencyValidator: normalizeText(readFileSync(resultLatencyValidatorPath, "utf8")),
    resultRetryValidator: normalizeText(readFileSync(resultRetryValidatorPath, "utf8")),
    resultResourceValidator: normalizeText(readFileSync(resultResourceValidatorPath, "utf8")),
    resultSecurityValidator: normalizeText(readFileSync(resultSecurityValidatorPath, "utf8")),
    resultRenderValidator: normalizeText(readFileSync(resultRenderValidatorPath, "utf8")),
    resultSelectionValidator: normalizeText(readFileSync(resultSelectionValidatorPath, "utf8")),
    resultCanonicalValidator: normalizeText(readFileSync(resultValidatorPath, "utf8")),
    sharedJcsRuntime: normalizeText(readFileSync(sharedJcsRuntimePath, "utf8")),
    sharedSchemaRuntime: normalizeText(readFileSync(sharedSchemaRuntimePath, "utf8")),
  });

const proposalFields = ["version", "role", "state", "wireProtocol", "modelId",
  "modelRevision", "endpointUrl", "credentialRef"];

function validateProviderEndpoint(endpointUrl, credentialKind, label) {
  if (Buffer.byteLength(endpointUrl, "utf8") > 2048 || /[^\x21-\x7e]/u.test(endpointUrl)) {
    throw new Error(`${label} endpoint is outside its ASCII/2KiB bounds`);
  }
  let endpoint;
  try {
    endpoint = new URL(endpointUrl);
  } catch {
    throw new Error(`${label} endpoint is not a URL`);
  }
  if (endpoint.href !== endpointUrl) {
    throw new Error(`${label} endpoint is not canonical`);
  }
  if (endpoint.username || endpoint.password) {
    throw new Error(`${label} endpoint contains userinfo`);
  }
  if (endpointUrl.includes("?") || endpointUrl.includes("#")) {
    throw new Error(`${label} endpoint contains a query or fragment`);
  }
  if (!endpoint.hostname || !["http:", "https:"].includes(endpoint.protocol) ||
      endpoint.pathname === "/") {
    throw new Error(`${label} endpoint is not a complete request URL`);
  }
  const host = endpoint.hostname;
  if (host.endsWith(".")) {
    throw new Error(`${label} endpoint uses a trailing-dot hostname`);
  }
  const local = host === "127.0.0.1" || host === "[::1]";
  const rejectedLocalAlias = host.includes("*") || host === "localhost" ||
    host.endsWith(".localhost") || host === "0.0.0.0" || host === "[::]" ||
    host === "[::ffff:0:0]" ||
    (host.startsWith("127.") && host !== "127.0.0.1") || host.startsWith("[::ffff:7f");
  if (rejectedLocalAlias) {
    throw new Error(`${label} endpoint hostname is not an accepted literal loopback or remote host`);
  }
  if (local && endpoint.protocol === "http:" && credentialKind !== "none") {
    throw new Error(`${label} local HTTP endpoint must be credential-none`);
  }
  return {
    endpoint,
    local,
    activationRejectionReason:
      !local && endpoint.protocol !== "https:" ? "insecure_remote_transport" : null,
  };
}

function validateProviderProposal(proposal, label) {
  if (
    Object.keys(proposal).sort().join("\0") !== [...proposalFields].sort().join("\0") ||
    proposal.version !== 1 || proposal.role !== "summary" || proposal.state !== "enabled" ||
    !["anthropic_messages_v1", "openai_chat_completions_v1"].includes(proposal.wireProtocol)
  ) {
    throw new Error(`${label} ProviderProposal is not the closed v1 shape`);
  }
  for (const [name, maxBytes] of [["modelId", 256], ["modelRevision", 128]]) {
    if (Buffer.byteLength(proposal[name], "utf8") === 0 ||
        Buffer.byteLength(proposal[name], "utf8") > maxBytes) {
      throw new Error(`${label} model field is outside its UTF-8 byte bounds`);
    }
    if (/[\u0000-\u001f\u007f]/u.test(proposal[name])) {
      throw new Error(`${label} model field contains an ASCII control character`);
    }
  }
  const credential = proposal.credentialRef;
  const credentialKeys = credential && Object.keys(credential).sort().join("\0");
  if (
    !credential ||
    (credential.kind === "none" && credentialKeys !== "kind") ||
    (credential.kind === "environment" &&
      (credentialKeys !== "kind\0name" ||
        !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/u.test(credential.name))) ||
    !["none", "environment"].includes(credential.kind)
  ) {
    throw new Error(`${label} credential reference is not closed none/environment v1`);
  }
  return validateProviderEndpoint(proposal.endpointUrl, credential.kind, label);
}

function validateProviderChoice(choice, label) {
  const proposal = Object.fromEntries(proposalFields.map((name) => [name, choice[name]]));
  const compiled = validateProviderProposal(proposal, label);
  if (compiled.activationRejectionReason !== null) {
    throw new Error(`${label} provider violates remote HTTPS policy`);
  }
  const expectedPolicy = compiled.local
    ? {
        executionLocation: "local",
        egressPolicy: "on_device",
        costClass: "local_zero",
        tlsPolicy: compiled.endpoint.protocol === "https:" ? "system" : "not_applicable",
      }
    : {
        executionLocation: "remote",
        egressPolicy: "explicit_remote",
        costClass: "external_metered",
        tlsPolicy: "system",
      };
  for (const [name, value] of Object.entries(expectedPolicy)) {
    if (choice[name] !== value) {
      throw new Error(`${label} provider ${name} is not compiler-derived from its endpoint`);
    }
  }
  const { providerFingerprint: _providerFingerprint, ...fingerprintInput } = choice;
  const actualProviderFingerprint = fingerprint(
    "free-mem:provider-choice:v1\0", fingerprintInput,
  );
  if (choice.providerFingerprint !== actualProviderFingerprint) {
    throw new Error(`${label} provider fingerprint does not match its closed choice`);
  }
}

function validateManifest(manifest, label, baseFingerprint = null) {
  validateProviderChoice(manifest.summaryProvider, label);
  if (
    (baseFingerprint === null && Object.hasOwn(manifest, "baseConfigurationFingerprint")) ||
    (baseFingerprint !== null && manifest.baseConfigurationFingerprint !== baseFingerprint)
  ) {
    throw new Error(`${label} manifest predecessor binding is invalid`);
  }
  const { configurationFingerprint: _configurationFingerprint, ...fingerprintInput } = manifest;
  const actualConfigurationFingerprint = fingerprint(
    "free-mem:effective-capability-manifest:v1\0", fingerprintInput,
  );
  if (manifest.configurationFingerprint !== actualConfigurationFingerprint) {
    throw new Error(`${label} manifest fingerprint does not match its non-secret configuration`);
  }
  return actualConfigurationFingerprint;
}

const actualConfigurationFingerprint = validateManifest(
  fixture.effectiveConfiguration, "effective",
);
validateManifest(
  fixture.localDerivationManifest, "local derivation", actualConfigurationFingerprint,
);
const actualRepairedConfigurationFingerprint = validateManifest(
  fixture.repairedRemoteManifest, "repaired remote", actualConfigurationFingerprint,
);
const actualRecoveryConfigurationFingerprint = validateManifest(
  fixture.outputLimitRecoveryManifest, "output-limit recovery", actualConfigurationFingerprint,
);
const outputLimitScenario = fixture.scenarios.find(
  (scenario) => scenario.fault?.kind === "summary_provider_output_limit_exceeded",
);
const recoverySignal = outputLimitScenario?.fault?.resumeCases.find(
  (item) => item.caseId === "validated-larger-limit-activation",
)?.signals[0];
if (
  fixture.outputLimitRecoveryManifest.manifestId !== outputLimitScenario?.fault?.recoveryManifestId ||
  fixture.outputLimitRecoveryManifest.resourceProfile.maxMemoryItemsPerDerivation !==
    outputLimitScenario.fault.observedResultCount ||
  recoverySignal?.providerFingerprint !==
    fixture.outputLimitRecoveryManifest.summaryProvider.providerFingerprint ||
  recoverySignal?.effectiveManifestFingerprint !== actualRecoveryConfigurationFingerprint
) {
  throw new Error("output-limit recovery manifest is not fully bound to its activation signal");
}

const retryScenario = fixture.scenarios.find(
  (scenario) => scenario.fault?.kind === "summary_provider_malformed_response",
);
const repairedSignals = [
  retryScenario?.fault?.resumeCases.find(
    (item) => item.caseId === "validated-configuration-activation",
  )?.signals[0],
  ...fixture.scenarios
    .filter((scenario) => scenario.fault?.kind === "summary_provider_redirect_response")
    .map((scenario) => scenario.fault.redirectRecovery.signal),
];
if (!repairedSignals.every((signal) =>
  signal?.providerFingerprint === fixture.repairedRemoteManifest.summaryProvider.providerFingerprint &&
  signal?.effectiveManifestFingerprint === actualRepairedConfigurationFingerprint
)) {
  throw new Error("provider recovery signals are not bound to the repaired remote manifest");
}

for (const scenario of fixture.scenarios) {
  const signals = [
    ...(scenario.fault?.resumeCases ?? []).flatMap((item) => item.signals),
    ...(scenario.fault?.redirectRecovery?.signal ? [scenario.fault.redirectRecovery.signal] : []),
  ];
  const producerBySignalId = new Map();
  const signalIdByProducer = new Map();
  if (signals.length > 0 && (!scenario.fault?.targetJobId || !signals.every((signal) => {
    const prior = producerBySignalId.get(signal.signalId);
    const priorSignalId = signalIdByProducer.get(signal.producerReceiptId);
    producerBySignalId.set(signal.signalId, signal.producerReceiptId);
    signalIdByProducer.set(signal.producerReceiptId, signal.signalId);
    return signal.targetJobId === scenario.fault.targetJobId &&
      typeof signal.producerReceiptId === "string" && signal.producerReceiptId.length > 0 &&
      (prior === undefined || prior === signal.producerReceiptId) &&
      (priorSignalId === undefined || priorSignalId === signal.signalId);
  }))) {
    throw new Error(`${scenario.scenarioId} resume signal is not bound to its job and producer receipt`);
  }
}

for (const scenario of fixture.scenarios.filter((item) => item.providerActivationProposal)) {
  const assessment = validateProviderProposal(
    scenario.providerActivationProposal.proposal, `${scenario.scenarioId} proposal`,
  );
  const transport = scenario.providerActivationProposal;
  const expectedReason = assessment.activationRejectionReason ??
    (transport.certificateChainState === "invalid" ? "tls_certificate_chain_invalid" : null) ??
    (transport.hostnameState === "mismatch" ? "tls_hostname_mismatch" : null);
  if (assessment.local || expectedReason !== scenario.summaryProviderStub.policyRejectedReason) {
    throw new Error(`${scenario.scenarioId} proposal rejection evidence is inconsistent`);
  }
}

const spool = fixture.scenarios.find(
  (scenario) => scenario.scenarioId === "runtime-unavailable-spool-recovery",
);
const probe = spool?.fault?.identityConflictProbe;
const canonicalEvent = spool?.events?.find((event) => event.eventId === probe?.eventId);

if (!probe || !canonicalEvent) {
  throw new Error("identity-conflict probe does not resolve its canonical event");
}
if (
  probe.repositoryScope !== spool.sourceRepositoryScope ||
  probe.source !== spool.canonicalEventSource ||
  probe.streamId !== spool.sourceStreamId ||
  typeof spool.canonicalEventSource !== "string" || spool.canonicalEventSource.length === 0 ||
  typeof spool.sourceStreamId !== "string" || spool.sourceStreamId.length === 0
) {
  throw new Error("identity-conflict probe does not bind canonical repository/source/stream identity");
}
if (probe.payloadDigestVersion !== canonicalEvent.payloadDigestVersion) {
  throw new Error("identity-conflict probe does not reuse the canonical digest version");
}
const expectedConflictReceiptId = `conflict-receipt-v1:${fingerprint(
  "free-mem:event-identity-conflict-receipt:v1\0",
  {
    repositoryScope: probe.repositoryScope,
    source: probe.source,
    streamId: probe.streamId,
    eventId: probe.eventId,
    payloadDigestVersion: probe.payloadDigestVersion,
    canonicalPayloadDigest: probe.canonicalPayloadDigest,
    conflictingPayloadDigest: probe.conflictingPayloadDigest,
  },
)}`;
if (
  probe.conflictReceiptId !== expectedConflictReceiptId ||
  !Array.isArray(probe.conflictAttemptReceiptIds) ||
  probe.conflictAttemptReceiptIds.length < 2 ||
  !probe.conflictAttemptReceiptIds.every((receiptId) => receiptId === expectedConflictReceiptId) ||
  probe.durableConflictReceiptCount !== 1
) {
  throw new Error("identity-conflict receipt is not unique to and reused for one digest pair");
}

if (
  fixture.contractFingerprint !== expectedContractFingerprint ||
  actualContractFingerprint !== expectedContractFingerprint
) {
  throw new Error("fixed fixture contract changed without a fixture-version fingerprint update");
}

const digestDomain = "free-mem:event-payload-digest:v1\0";
const digest = (payload) => fingerprint(digestDomain, payload);

const lineageVectors = [
  {
    spans: [{ eventId: "event-a", startByte: 0, endByte: 10 }],
    expected: "ca6ec88cc0156199c5e08e50393dcf4ef473f62c1e0e5372e99d9d791499779f",
  },
  {
    spans: [{ eventId: "event-a", startByte: 0, endByte: 11 }],
    expected: "ad0ffa99555520b5a1524908a4d08661b3054b0f0f8a1a66e08a8f2a98904378",
  },
  {
    spans: [
      { eventId: "event-b", startByte: 4, endByte: 9 },
      { eventId: "event-a", startByte: 0, endByte: 10 },
    ],
    expected: "23274e7fbe3af129f9942e312eec51dfdc6b6825e3e38583d068254e96e2d447",
  },
];
for (const vector of lineageVectors) {
  if (lineageDigest("repo-primary", vector.spans) !== vector.expected) {
    throw new Error("lineage v1 test vector mismatch");
  }
}

const isUtf8Boundary = (bytes, offset) =>
  Number.isInteger(offset) &&
  offset >= 0 &&
  offset <= bytes.length &&
  (offset === 0 || offset === bytes.length || (bytes[offset] & 0xc0) !== 0x80);

const utf8BoundaryProbe = {
  text: "設定",
  validByteOffsets: [0, 3, 6],
  invalidByteOffsets: [1, 2, 4, 5],
};
const boundaryProbeBytes = Buffer.from(utf8BoundaryProbe.text, "utf8");
if (
  !utf8BoundaryProbe.validByteOffsets.every((offset) =>
    isUtf8Boundary(boundaryProbeBytes, offset),
  ) ||
  !utf8BoundaryProbe.invalidByteOffsets.every(
    (offset) => !isUtf8Boundary(boundaryProbeBytes, offset),
  )
) {
  throw new Error("UTF-8 boundary probe mismatch");
}

for (const scenario of fixture.scenarios) {
  const events = new Map(scenario.events.map((event) => [event.eventId, event]));
  for (const item of [...scenario.expectedInjectedItems, ...scenario.expectedOmissions]) {
    if (item.lineageId !== lineageDigest(scenario.sourceRepositoryScope, item.sourceSpans)) {
      throw new Error(`candidate lineage does not match source evidence in ${scenario.scenarioId}`);
    }
  }
  const outputs = [
    scenario.summaryProviderStub.summary,
    ...scenario.summaryProviderStub.memoryItems,
    scenario.fault?.recoveredOutput?.summary,
    ...(scenario.fault?.recoveredOutput?.memoryItems ?? []),
    ...scenario.expectedInjectedItems,
    ...scenario.expectedOmissions,
  ].filter(Boolean);
  for (const output of outputs) {
    for (const span of output.sourceSpans) {
      const event = events.get(span.eventId);
      const bytes = event && Buffer.from(event.redactedPayload, "utf8");
      if (
        !bytes ||
        !isUtf8Boundary(bytes, span.startByte) ||
        !isUtf8Boundary(bytes, span.endByte) ||
        span.startByte >= span.endByte
      ) {
        throw new Error(`invalid UTF-8 source span in scenario ${scenario.scenarioId}`);
      }
    }
  }
}

if (digest(canonicalEvent.redactedPayload) !== probe.canonicalPayloadDigest) {
  throw new Error("identity-conflict canonical digest does not match redacted payload");
}
if (
  probe.conflictingRedactedPayload === canonicalEvent.redactedPayload ||
  digest(probe.conflictingRedactedPayload) !== probe.conflictingPayloadDigest
) {
  throw new Error("identity-conflict payload or digest is not a distinct reproducible input");
}

try {
  execFileSync("jq", ["-e", "-f", semanticPath], {
    input: JSON.stringify(fixture),
    stdio: ["pipe", "inherit", "inherit"],
  });
} catch (error) {
  if (error?.code === "ENOENT") {
    console.error("Prerequisite missing: jq is required to validate the Slice 1 fixture.");
    process.exit(2);
  }
  throw error;
}
