def ensure($condition; $message):
  if $condition then . else error($message) end;

def fault_scenario($root; $kind):
  [ $root.scenarios[] | select(.fault?.kind == $kind) ]
  | if length == 1 then .[0] else null end;

def output_items($output):
  (if ($output | has("summary"))
   then [{
     "body": $output.summary.body,
     "kind": "summary",
     "sourceEventIds": $output.summary.sourceEventIds,
     "sourceSpans": $output.summary.sourceSpans
   }]
   else []
   end) + ($output.memoryItems | map({body, kind, sourceEventIds, sourceSpans}));

def spans_overlap($left; $right):
  $left.eventId == $right.eventId
  and $left.startByte < $right.endByte
  and $right.startByte < $left.endByte;

def output_anchors_disjoint($output):
  output_items($output) as $items
  | all($items[]; .sourceSpans as $spans
    | all(range(0; ($spans | length)); . as $leftIndex
      | all(range($leftIndex + 1; ($spans | length)); . as $rightIndex
        | (spans_overlap($spans[$leftIndex]; $spans[$rightIndex]) | not))))
  and all(range(0; ($items | length)); . as $leftIndex
    | all(range($leftIndex + 1; ($items | length)); . as $rightIndex
      | all($items[$leftIndex].sourceSpans[]; . as $left
        | all($items[$rightIndex].sourceSpans[]; . as $right
          | (spans_overlap($left; $right) | not)))));

def sensitivity_rank:
  if . == "eligible" then 0
  elif . == "local_only" then 1
  elif . == "private" then 2
  elif . == "secret" then 3
  else error("unknown sensitivity")
  end;

def derived_sensitivity($scenario; $sourceEventIds):
  ([
    $sourceEventIds[]
    | . as $sourceId
    | $scenario.events[]
    | select(.eventId == $sourceId)
    | .sensitivity
  ] | max_by(sensitivity_rank));

def derived_output_items($scenario; $output):
  output_items($output)
  | map(. + {sensitivity: derived_sensitivity($scenario; .sourceEventIds)});

def provider_items($scenario):
  derived_output_items($scenario; $scenario.summaryProviderStub);

def output_sources_ok($scenario; $output):
  all(output_items($output)[];
    . as $item
    | (.sourceEventIds | length) == (.sourceEventIds | unique | length)
    and ([.sourceSpans[] | [.eventId, .startByte, .endByte] | @json]
      | length == (unique | length))
    and ([.sourceSpans[].eventId] | unique | sort) ==
      (.sourceEventIds | unique | sort)
    and all(.sourceEventIds[];
      . as $sourceId
      | any($scenario.events[]; .eventId == $sourceId))
    and all($item.sourceSpans[];
      . as $span
      | ([ $scenario.events[] | select(.eventId == $span.eventId) ] | length) == 1
      and ($scenario.events[]
        | select(.eventId == $span.eventId)
        | (.redactedPayload | utf8bytelength) as $payloadBytes
        | $span.startByte >= 0
          and $span.startByte < $span.endByte
          and $span.endByte <= $payloadBytes)));

def provider_item_key:
  [ .kind, .body, .sourceEventIds, .sourceSpans, .sensitivity ] | @json;

def expected_item_key:
  [ .memoryKind, .fact, .sourceEventIds, .sourceSpans, .sensitivity ] | @json;

def revision_identity_ok($scenario):
  ([ $scenario.expectedInjectedItems[], $scenario.expectedOmissions[] ]) as $items
  | ([ $items[] | select(.reason? != "duplicate_revision") ]) as $normal
  | all(["lineageId", "memoryId", "revisionId"][];
      . as $field
      | ([ $normal[][$field] ] | length == (unique | length)))
    and all($items[] | select(.reason? == "duplicate_revision");
      . as $duplicate
      | any($scenario.expectedInjectedItems[];
        del(.selectionReason) == ($duplicate | del(.reason))));

def fixture_graph_ok($root):
  ($root.scenarios | map(.scenarioId)) as $scenarioIds
  | ($root.scenarios | map(select((.events | length) > 0) | .scenarioId)) as $captureScenarioIds
  | ($scenarioIds | length) == ($scenarioIds | unique | length)
  and $root.samplingProtocol.runsPerScenario ==
    ($root.samplingProtocol.discardInitialRunsPerScenario +
      $root.samplingProtocol.measuredRunsPerScenario)
  and $root.samplingProtocol.percentileMethod == "nearest_rank_ceiling"
  and $root.samplingProtocol.percentileScope == "per_scenario_no_pooling"
  and $root.samplingProtocol.clock == "monotonic"
  and ([ $root.samplingProtocol.metrics.captureP95Ms.scenarios[] ] | sort) ==
    ($captureScenarioIds | sort)
  and all($root.samplingProtocol.metrics.warmInjectionP95Ms.scenarios[];
    . as $scenarioId
    | any($root.scenarios[];
        .scenarioId == $scenarioId and .resourceSampleMode == "warm"
        and .drainCondition.targetInjectionAcknowledged
        and (.expectedInjectedItems | length) > 0))
  and all($root.samplingProtocol.metrics.shortColdLexicalInjectionMs.scenarios[];
    . as $scenarioId
    | any($root.scenarios[];
        .scenarioId == $scenarioId and .resourceSampleMode == "cold"
        and .drainCondition.targetInjectionAcknowledged
        and (.expectedInjectedItems | length) > 0))
  and all($root.samplingProtocol.metrics[].scenarios[];
    . as $scenarioId
    | ($scenarioIds | index($scenarioId)) != null)
  and (($root.lifecycleProfiles | keys) as $profiles
    | all($profiles[];
      . as $profileId
      | any($root.scenarios[]; .lifecycleProfileId == $profileId)));

def host_identity_ok($root):
  $root.hostIdentityProbe as $probe
  | $probe.hostObservedIdentity as $host
  | ([ $root.scenarios[] | select(.scenarioId == $probe.scenarioId) ]
      | if length == 1 then .[0] else null end) as $scenario
  | $scenario != null
    and $scenario.sourceAgent == $host.agent
    and $scenario.sourceRepositoryScope == $host.repositoryScope
    and [ $probe.callerClaimProbes[].mismatchField ] == [
      "agent", "repositoryScope", "sessionId"
    ]
    and all($probe.callerClaimProbes[];
      . as $claim
      | (["agent", "repositoryScope", "sessionId"]
        | map(select($claim.callerClaimedIdentity[.] != $host[.]))) ==
          [$claim.mismatchField])
    and $probe.expectedResult.consideredClaimCount ==
      ($probe.callerClaimProbes | length)
    and $probe.expectedResult.effectiveIdentity == $host
    and [ $probe.expectedResult.decisions[].probeId ] ==
      [ $probe.callerClaimProbes[].probeId ]
    and all($probe.expectedResult.decisions[];
      (.authorityAccepted | not) and .reason == "caller_claim_discarded");

def provider_transmission_ok($root):
  all($root.scenarios[];
      . as $scenario
      | $scenario.providerTransmissionOracle as $wire
      | ((($scenario.summaryProviderStub | has("summary"))
            or ($scenario.summaryProviderStub | has("malformedResponse"))
            or ($scenario.summaryProviderStub | has("redirectResponse"))
            or ($scenario.summaryProviderStub.memoryItems | length) > 0)
          and ($scenario.summaryProviderStub | has("policyRejectedReason") | not)) as $providerAttempt
      | (($scenario.derivationManifestId? // "") ==
          $root.localDerivationManifest.manifestId) as $localAttempt
      | if $providerAttempt
        then (if $localAttempt
            then $wire.credentialBytesSent == 0
            else $wire.credentialBytesSent > 0
            end)
          and $wire.payloadBytesSent > 0
          and $wire.completionMilestone != null
          and ($root.lifecycleProfiles[$scenario.lifecycleProfileId] as $profile
            | ($profile | index($scenario.drainCondition.startMilestone)) as $start
            | ($profile | index($wire.completionMilestone)) as $completion
            | ($profile | index($scenario.drainCondition.terminalMilestone)) as $terminal
            | $start != null and $completion != null and $terminal != null
              and $start < $completion and $completion <= $terminal)
        else $wire.credentialBytesSent == 0 and $wire.payloadBytesSent == 0
          and $wire.completionMilestone == null
        end);

def legacy_dispositions_ok($manifest):
  [ $manifest.legacyDispositions[].key ] as $keys
  | $keys == ($keys | sort)
    and ($keys | length) == ($keys | unique | length);

def provider_choice_ok($provider; $endpointUrl; $credentialRef; $location; $egress; $cost; $tls):
  $provider.version == 1
    and $provider.role == "summary"
    and $provider.state == "enabled"
    and $provider.wireProtocol == "openai_chat_completions_v1"
    and $provider.endpointUrl == $endpointUrl
    and $provider.credentialRef == $credentialRef
    and $provider.executionLocation == $location
    and $provider.egressPolicy == $egress
    and $provider.costClass == $cost
    and $provider.tlsPolicy == $tls
    and $provider.redirectPolicy == "reject";

def manifest_contract_ok($root):
  $root.effectiveConfiguration as $base
  | $root.localDerivationManifest as $local
  | $root.repairedRemoteManifest as $repaired
  | $root.outputLimitRecoveryManifest as $output
  | ($base | has("baseConfigurationFingerprint") | not)
    and $base.manifestId == "slice1-effective-manifest-v1"
    and $local.manifestId == "slice1-local-summary-manifest-v1"
    and $repaired.manifestId == "slice1-repaired-remote-manifest-v1"
    and $output.manifestId == "slice1-output-limit-recovery-manifest-v1"
    and all([$local, $repaired, $output][];
      .baseConfigurationFingerprint == $base.configurationFingerprint)
    and all([$base, $local, $repaired, $output][]; legacy_dispositions_ok(.))
    and provider_choice_ok($base.summaryProvider;
      "https://summary.stub.invalid/v1/chat/completions";
      {"kind":"environment","name":"FREE_MEM_SUMMARY_API_KEY"};
      "remote"; "explicit_remote"; "external_metered"; "system")
    and provider_choice_ok($local.summaryProvider;
      "https://127.0.0.1:1234/v1/chat/completions";
      {"kind":"none"}; "local"; "on_device"; "local_zero"; "system")
    and provider_choice_ok($repaired.summaryProvider;
      "https://summary-repaired.stub.invalid/v1/chat/completions";
      {"kind":"environment","name":"FREE_MEM_SUMMARY_API_KEY"};
      "remote"; "explicit_remote"; "external_metered"; "system")
    and $output.summaryProvider == $base.summaryProvider
    and all([$local, $repaired, $output][];
      .destinationPolicyMap == $base.destinationPolicyMap
        and .embeddingProvider == $base.embeddingProvider
        and .legacyDispositions == $base.legacyDispositions);

def output_limit_recovery_manifest_ok($root):
  $root.outputLimitRecoveryManifest as $recovery
  | $root.effectiveConfiguration as $base
  | fault_scenario($root; "summary_provider_output_limit_exceeded") as $scenario
  | ($scenario.fault.resumeCases[]
      | select(.caseId == "validated-larger-limit-activation")
      | .signals[0]) as $signal
  | $recovery.baseConfigurationFingerprint == $base.configurationFingerprint
    and $recovery.manifestId == $scenario.fault.recoveryManifestId
    and $recovery.configurationFingerprint == $signal.effectiveManifestFingerprint
    and $recovery.configurationFingerprint != $base.configurationFingerprint
    and $recovery.resourceProfile.version == ($base.resourceProfile.version + 1)
    and $recovery.resourceProfile.maxMemoryItemsPerDerivation ==
      $scenario.fault.observedResultCount
    and ($recovery
      | del(.manifestId, .baseConfigurationFingerprint, .configurationFingerprint,
          .resourceProfile.version,
          .resourceProfile.maxMemoryItemsPerDerivation)) ==
      ($base
      | del(.manifestId, .configurationFingerprint, .resourceProfile.version,
          .resourceProfile.maxMemoryItemsPerDerivation));

def before_model_evidence_ok($root):
  all($root.scenarios[] | select(.drainCondition.targetInjectionAcknowledged);
    . as $scenario
    | $root.lifecycleProfiles[$scenario.lifecycleProfileId] as $profile
    | ($profile | index("target_injection_acknowledged")) as $injectionIndex
    | ($profile | index("target_model_request_dispatched")) as $dispatchIndex
    | $injectionIndex != null
      and $dispatchIndex != null
      and $injectionIndex < $dispatchIndex)
  and ($root.beforeModelNegativeFixture as $negative
    | any($root.scenarios[];
        .scenarioId == $negative.baseScenarioId
        and .drainCondition.targetInjectionAcknowledged)
      and $negative.nonBeforeModelMilestones == [
        "target_model_request_dispatched",
        "target_injection_acknowledged"
      ]
      and ($negative.injectionBeforeModel | not)
      and ($negative.expectedDisposition == {
        state: "failed",
        reason: "scenario_oracle_mismatch",
        successfulComparisonEligible: false
      }));

def injection_envelope_ok($root):
  $root.effectiveConfiguration.resourceProfile.injectionEnvelope as $envelope
  | all($envelope.laneBudgets[]; .minItems <= .maxItems)
  and $envelope.maxSelectedItems <= $envelope.admittedCandidateLimit
  and $envelope.maxInjectedTokens == $root.thresholds.maxInjectedTokens
  and $envelope.selectionTimeBudgetMs < $root.thresholds.warmInjectionP95Ms
  and (if $root.effectiveConfiguration.embeddingProvider.state == "disabled"
    then $envelope.laneBudgets.semantic.maxItems == 0
    else true
    end)
  and all($root.scenarios[];
    . as $scenario
    | (.expectedInjectedItems | length) <= $envelope.maxSelectedItems
    and all($envelope.laneBudgets | keys[];
      . as $lane
      | ([ $scenario.expectedInjectedItems[] | select(.sourceLane == $lane) ] | length)
        <= $envelope.laneBudgets[$lane].maxItems));

def resource_profile_ok($root):
  $root.effectiveConfiguration.resourceProfile as $profile
  | $profile.processingQueueCapacity >= $profile.resourceWarningThresholds.maxPendingQueueDepth
  and $profile.maxSourceEventsPerJob == 100
  and $profile.observerRequestTimeoutMs == 60000
  and $profile.observerMaxInputChars == 12000
  and $profile.observerMaxOutputTokens == 4000
  and $profile.observerMaxResponseBytes == 1048576
  and $profile.observerTemperature == 0.2
  and $profile.providerTlsPreflightTimeoutMs == 5000
  and $profile.periodicSweepIntervalMs == 30000
  and $profile.idleFlushMs == 120000
  and $profile.eventDebounceMs == 1000
  and $profile.stuckClaimTimeoutMs == 300000
  and ($profile.rawEventRetentionEnabled | not)
  and $profile.rawEventRetentionMs == 0
  and $root.localDerivationManifest.resourceProfile == $profile
  and $root.repairedRemoteManifest.resourceProfile == $profile
  and $profile.resourceWarningThresholds == {
    "maxSteadyProductProcessCount": $root.thresholds.maxSteadyProductProcessCount,
    "maxShortRunRssGrowthMiB": $root.thresholds.maxShortRunRssGrowthMiB,
    "maxPendingQueueDepth": $root.thresholds.maxPendingQueueDepth,
    "maxStorageGrowthBytes": $root.thresholds.maxStorageGrowthBytes
  }
  and all(["agentBlockageCount", "acceptedEventLossCount", "duplicateDurableMemoryCount",
    "secretEgressCount", "incompatibleScopeInjectionCount"][];
    . as $name | $root.thresholds[$name] == 0);

def destination_policy_ok($root):
  # Local destination classes are runner-only fixture evidence. Production Agents remain remote/unknown.
  $root.effectiveConfiguration.destinationPolicyMap as $policies
  | $root.effectiveConfiguration.manifestVersion == 1
  and $root.effectiveConfiguration.manifestId == "slice1-effective-manifest-v1"
  and $policies["claude-code-local"].targetAgent == "claude-code"
  and $policies["codex-local"].targetAgent == "codex"
  and $policies["claude-code-remote"].targetAgent == "claude-code"
  and $policies["codex-remote"].targetAgent == "codex"
  and all($root.scenarios[];
    . as $scenario
    | ($policies | has($scenario.targetDestinationClass)));

def selection_lifecycle_ok($root):
  all($root.scenarios[];
    . as $scenario
    | $root.lifecycleProfiles[$scenario.lifecycleProfileId] as $milestones
    | ($milestones | index("target_selection_started")) as $started
    | ($milestones | index("target_selection_finished")) as $finished
    | if $scenario.drainCondition.targetInjectionAcknowledged
      then ($milestones | index("target_retrieval_requested")) as $requested
        | $requested != null
        and $requested < $started
        and $started < $finished
        and $finished < ($milestones | index("target_injection_acknowledged"))
        and ($milestones | index("target_injection_acknowledged")) <
          ($milestones | index("target_model_request_dispatched"))
      else $started == null and $finished == null
      end);

def resource_metrics_ok($root):
  all($root.samplingProtocol.resourceMetrics[];
    . as $metric
    | all($root.scenarios[];
      $root.lifecycleProfiles[.lifecycleProfileId] as $milestones
      | ($milestones | index($metric.startMilestone)) as $start
      | ($milestones | index($metric.endMilestone)) as $end
      | $start != null and $end != null and $start < $end));

def failure_continuation_ok($root):
  all($root.scenarios[] | select(has("expectedOperationalStatus"));
    $root.lifecycleProfiles[.lifecycleProfileId] as $milestones
    | ($milestones | index("target_injection_skipped")) as $skipped
    | ($milestones | index("target_model_continued_after_memory_failure")) as $continued
    | $skipped != null and $continued != null and $skipped < $continued);

def pack_degradation_policy_ok($root):
  if $root.effectiveConfiguration.embeddingProvider.state == "disabled"
  then $root.effectiveConfiguration.embeddingProvider.packDegradationReason == "semantic_disabled"
  else true
  end;

def transport_security_ok($root):
  ([ $root.scenarios[] | select(has("providerActivationProposal")) ]) as $matches
  | ($matches | length) == 4
  and ([$matches[].scenarioId] | sort) == [
    "credentialed-http-activation-rejected",
    "credentialless-http-activation-rejected",
    "hostname-mismatch-https-activation-rejected",
    "invalid-chain-https-activation-rejected"
  ]
  and ([$matches[].providerActivationProposal.proposal.credentialRef.kind == "environment"] | sort) ==
    [false, true, true, true]
  and all($matches[];
    . as $scenario
    | $scenario.providerActivationProposal.proposal as $proposal
    | (if ($proposal.endpointUrl | startswith("http://"))
      then "insecure_remote_transport"
      elif $scenario.providerActivationProposal.certificateChainState == "invalid"
      then "tls_certificate_chain_invalid"
      elif $scenario.providerActivationProposal.hostnameState == "mismatch"
      then "tls_hostname_mismatch"
      else null
      end) as $expectedReason
    | $scenario.lifecycleProfileId == "configuration_rejection"
    and ($scenario.events | length) == 0
    and $proposal.version == 1
    and $proposal.role == "summary"
    and $proposal.state == "enabled"
    and $proposal.wireProtocol == "openai_chat_completions_v1"
    and (if ($proposal.endpointUrl | startswith("http://"))
      then $scenario.providerActivationProposal.certificateChainState == "not_applicable"
        and $scenario.providerActivationProposal.hostnameState == "not_applicable"
      else ($proposal.endpointUrl | startswith("https://"))
        and (($scenario.providerActivationProposal.certificateChainState == "invalid"
            and $scenario.providerActivationProposal.hostnameState == "valid")
          or ($scenario.providerActivationProposal.certificateChainState == "valid"
            and $scenario.providerActivationProposal.hostnameState == "mismatch"))
      end)
    and $scenario.providerActivationProposal.payloadBytes ==
      ($scenario.providerActivationProposal.redactedPayload | utf8bytelength)
    and $scenario.providerActivationProposal.payloadBytes > 0
    and $expectedReason != null
    and $scenario.summaryProviderStub.policyRejectedReason == $expectedReason
    and $scenario.securityOracle.consideredActivationProposalCount == 1
    and $scenario.securityOracle.expectedActivationState == "rejected"
    and $scenario.securityOracle.expectedReason == $expectedReason
    and $scenario.securityOracle.remoteProviderRequestCount == 0
    and $scenario.securityOracle.credentialBytesSent == 0
    and $scenario.securityOracle.payloadBytesSent == 0
    and $scenario.drainCondition.providerRequestCount == 0
    and $scenario.drainCondition.providerPayloadCount == 0
    and ($scenario.securityOracle.forbiddenSentinels[0] as $sentinel
      | $scenario.providerActivationProposal.redactedPayload | contains($sentinel))
    and ($scenario.securityOracle.sentinelObservedAtRemote | not));

def scenario_core_ok($root; $scenario):
  [ $scenario.events[].sequence ] == [ range(1; (($scenario.events | length) + 1)) ]
  and (if ($scenario | has("providerActivationProposal"))
    then ($scenario.events | length) == 0
    else ($scenario.events | length) > 0
    end)
  and all($scenario.events[];
    if .sensitivity == "secret"
    then .redactedPayload == ""
    else .redactedPayload == .text
    end)
  and ($root.lifecycleProfiles | has($scenario.lifecycleProfileId))
  and ($root.lifecycleProfiles[$scenario.lifecycleProfileId]
    | length == (unique | length))
  and (($root.lifecycleProfiles[$scenario.lifecycleProfileId]
    | index($scenario.drainCondition.startMilestone)) as $start
    | ($root.lifecycleProfiles[$scenario.lifecycleProfileId]
      | index($scenario.drainCondition.terminalMilestone)) as $end
    | $start != null and $end != null and $start < $end)
  and $scenario.drainCondition.committedEventCount == ($scenario.events | length);

def common_scenarios_ok($root):
  ([ $root.scenarios[].events[].eventId ] as $ids
    | ($ids | length) == ($ids | unique | length))
  and all($root.scenarios[] | select(.fault?.kind != "summary_provider_output_limit_exceeded");
    . as $scenario
    | scenario_core_ok($root; $scenario)
    and revision_identity_ok($scenario)
    and .drainCondition.summaryCount ==
      (if (.summaryProviderStub | has("summary")) then 1 else 0 end)
    and .drainCondition.durableMemoryCount ==
      (.drainCondition.summaryCount + (.summaryProviderStub.memoryItems | length))
    and (provider_items($scenario) as $providerItems
      | output_sources_ok($scenario; $scenario.summaryProviderStub)
      and output_anchors_disjoint($scenario.summaryProviderStub)
      and ($providerItems | length) <=
          $root.effectiveConfiguration.resourceProfile.maxMemoryItemsPerDerivation
      and ([$providerItems[].sourceSpans | @json]
        | length == (unique | length))
      and ([ .expectedInjectedItems[].fact ] | length == (unique | length))
      and ([ .expectedOmissions[].fact ] | length == (unique | length))
      and all($scenario.forbiddenFacts[];
        . as $forbidden
        | all($scenario.expectedInjectedItems[];
          (.fact | contains($forbidden)) | not))
      and all(.expectedInjectedItems[];
        (.sourceLane == "exact_session" or .sourceLane == "lexical")
        and .selectionReason == .sourceLane)
      and all(.expectedOmissions[];
        .reason == "duplicate_revision"
        or .reason == "omitted_budget"
        or .reason == "omitted_ineligible")
      and (([ .expectedInjectedItems[].fact ] + [ .expectedOmissions[].fact ])
        | length == (unique | length))
      and ((.expectedInjectedItems | length) + (.expectedOmissions | length)
        == ($providerItems | length))
      # Provider extraction order is not injection order. This multiset check binds membership;
      # expectedInjectedItems order is pinned by the whole-fixture fingerprint and runtime oracle.
      and ([$providerItems[] | provider_item_key] | sort) ==
        (([.expectedInjectedItems[] | expected_item_key]
          + [.expectedOmissions[] | expected_item_key]) | sort)
      and all(.expectedInjectedItems[];
        . as $item
        | ([
            $providerItems[]
            | select(.body == $item.fact
                and .kind == $item.memoryKind
                and .sourceEventIds == $item.sourceEventIds
                and .sourceSpans == $item.sourceSpans
                and .sensitivity == $item.sensitivity)
          ] | length) == 1
        and ($item.sourceEventIds | length == (unique | length))
        and all($item.sourceEventIds[];
          . as $sourceId
          | any($scenario.events[]; .eventId == $sourceId)))
      and all(.expectedOmissions[];
        . as $item
        | ([
            $providerItems[]
            | select(.body == $item.fact
                and .kind == $item.memoryKind
                and .sourceEventIds == $item.sourceEventIds
                and .sourceSpans == $item.sourceSpans
                and .sensitivity == $item.sensitivity)
          ] | length) == 1
        and ($item.sourceEventIds | length == (unique | length))
        and all($item.sourceEventIds[];
          . as $sourceId
          | any($scenario.events[]; .eventId == $sourceId)))));

def bidirectional_ok($root):
  ([
    $root.scenarios[]
    | select((has("fault") | not)
        and .sourceRepositoryScope == .targetRepositoryScope
        and (.expectedInjectedItems | length) > 0)
  ]) as $flows
  | ($flows | length) == 2
  and ([ $flows[] |
    (.sourceAgent + "->" +
      $root.effectiveConfiguration.destinationPolicyMap[.targetDestinationClass].targetAgent)
  ] | sort) ==
    (["claude-code->codex", "codex->claude-code"] | sort)
  and all($flows[];
    $root.lifecycleProfiles[.lifecycleProfileId] as $milestones
    | ($milestones | index("target_first_prompt_submitted_before_model")) as $prompt
    | ($milestones | index("source_summary_committed")) as $summary
    | $prompt != null and $summary != null and $prompt < $summary)
  and any($flows[];
    . as $flow
    | any($flow.expectedInjectedItems[] | select(.memoryKind == "failed_approach");
      . as $item
      | any($item.sourceEventIds[];
        . as $sourceId
        | any($flow.events[];
          .eventId == $sourceId and .kind == "assistant_message"))));

def spool_ok($root):
  fault_scenario($root; "daemon_unavailable_after_event_accept")
  | [ .events[].eventId ] as $eventIds
  | .fault.identityConflictProbe as $probe
  | ($eventIds | length) == 2
    and ($eventIds | unique | length) == 2
    and .fault.recovery == "restart_and_replay_same_batch_twice"
    and (.fault.replaySchedule | length) == 2
    and [ .fault.replaySchedule[].attempt ] == [1, 2]
    and all(.fault.replaySchedule[]; .eventIds == $eventIds)
    and .drainCondition.spooledEventCount == ($eventIds | length)
    and .drainCondition.replayCount == 2
    and .canonicalEventSource == "claude"
    and .sourceStreamId == "runtime-unavailable-spool-recovery:source-stream"
    and .fault.identityConflictProbe.repositoryScope == .sourceRepositoryScope
    and .fault.identityConflictProbe.source == .canonicalEventSource
    and .fault.identityConflictProbe.streamId == .sourceStreamId
    and .fault.identityConflictProbe.eventId == $eventIds[1]
    and .fault.identityConflictProbe.canonicalPayloadDigest
      != .fault.identityConflictProbe.conflictingPayloadDigest
    and (.fault.identityConflictProbe.conflictReceiptId
      | test("^conflict-receipt-v1:sha256:[0-9a-f]{64}$"))
    and (.fault.identityConflictProbe.conflictAttemptReceiptIds | length) >= 2
    and all(.fault.identityConflictProbe.conflictAttemptReceiptIds[];
      . == $probe.conflictReceiptId)
    and .fault.identityConflictProbe.durableConflictReceiptCount == 1
    and .fault.identityConflictProbe.conflictReceiptState == "non_success"
    and .fault.identityConflictProbe.canonicalEventState == "committed"
    and .fault.identityConflictProbe.incomingDeliveryState == "quarantined"
    and .fault.identityConflictProbe.expectedReason == "event_identity_payload_conflict"
    and .fault.identityConflictProbe.canonicalPayloadUnchanged
    and .fault.identityConflictProbe.durableMemoryDelta == 0;

def resume_case_signal_sets_ok($case):
  ($case.expectedConsumedSignalIds | length) +
    ($case.expectedIgnoredSignalIds | length) == ($case.signals | length)
  and ($case.expectedIgnoredSignalIds | length) == $case.expected.ignoredSignalCount
  and (([$case.signals[].signalId] | sort) ==
    (([$case.expectedConsumedSignalIds[]] + [$case.expectedIgnoredSignalIds[]]) | sort));

def resume_transmission_ok($case; $credentialBytes; $payloadBytes):
  $case.expected.attemptDelta as $attempts
  | $case.expectedTransmissionEvidence == {
      "remoteProviderRequestCount": $attempts,
      "remoteProviderPayloadCount": $attempts,
      "credentialBytesSent": ($credentialBytes * $attempts),
      "payloadBytesSent": ($payloadBytes * $attempts),
      "restrictedPayloadBytesSent": 0,
      "forbiddenSentinelObservationCount": 0
    };

def successful_recovery_transition_ok(
  $signal; $expected; $consumed; $ignored; $kind; $providerFingerprint; $manifestFingerprint
):
  $signal.kind == $kind
    and $signal.providerFingerprint == $providerFingerprint
    and $signal.effectiveManifestFingerprint == $manifestFingerprint
    and $signal.sequence == 1
    and $expected.lastConsumedSequence == 1
    and $consumed == [$signal.signalId]
    and ($ignored | length) == 0
    and $expected.budgetBefore == 0
    and $expected.budgetAfterGrant == 1
    and $expected.budgetAfterAttempt == 0
    and $expected.attemptDelta == 1
    and $expected.ignoredSignalCount == 0;

def ignored_noop_transition_ok($case; $kind; $providerFingerprint; $manifestFingerprint):
  ($case.signals | length) == 1
    and $case.signals[0].kind == $kind
    and $case.signals[0].sequence == 1
    and $case.signals[0].providerFingerprint == $providerFingerprint
    and $case.signals[0].effectiveManifestFingerprint == $manifestFingerprint
    and $case.expectedConsumedSignalIds == []
    and $case.expectedIgnoredSignalIds == [$case.signals[0].signalId]
    and $case.providerOutcome == null
    and $case.expected.budgetBefore == 0
    and $case.expected.budgetAfterGrant == 0
    and $case.expected.budgetAfterAttempt == 0
    and $case.expected.attemptDelta == 0
    and $case.expected.lastConsumedSequence == 0
    and $case.expected.ignoredSignalCount == 1
    and $case.expected.finalState == "retry-exhausted"
    and $case.expected.durableMemoryCount == 0;

def retry_ok($root):
  fault_scenario($root; "summary_provider_malformed_response") as $retry
  | $retry.drainCondition.eventDeliveryState == "committed"
    and all($retry.fault.resumeCases[].signals[];
      .targetJobId == $retry.fault.targetJobId)
    and all($retry.fault.resumeCases[]; resume_case_signal_sets_ok(.))
    and all($retry.fault.resumeCases[];
      resume_transmission_ok(.;
        $retry.providerTransmissionOracle.credentialBytesSent /
          $retry.fault.attemptsUntilExhausted;
        $retry.providerTransmissionOracle.payloadBytesSent /
          $retry.fault.attemptsUntilExhausted))
    and $retry.drainCondition.summaryJobState == "retry-exhausted"
    and $retry.fault.attemptsUntilExhausted ==
      $root.effectiveConfiguration.resourceProfile.processingRetryLimit
    and $retry.fault.resumeCaseInitialSnapshot == {
      "state": "retry-exhausted",
      "budget": 0,
      "lastConsumedSequence": 0
    }
    and [ $retry.fault.resumeCases[].caseId ] == [
      "validated-configuration-activation",
      "recorded-provider-healthy-transition",
      "user-confirmed-doctor-retry",
      "duplicate-and-out-of-order-no-op"
    ]
    and ([
      $retry.fault.resumeCases[]
      | select(.providerOutcome == "valid")
      | .signals[0].kind
    ] | sort) == ([
      "validated_configuration_activation",
      "recorded_provider_healthy_transition",
      "user_confirmed_doctor_retry"
    ] | sort)
    and all($retry.fault.resumeCases[] | select(.providerOutcome == "valid");
      .expected.budgetBefore == 0
      and .signals[0].sequence == .expected.lastConsumedSequence
      and .expected.budgetAfterGrant == 1
      and .expected.budgetAfterAttempt == 0
      and .expected.attemptDelta == 1
      and .expected.lastConsumedSequence == 1
      and .expected.ignoredSignalCount == 0
      and .expected.finalState == "completed"
      and .expected.durableMemoryCount ==
        (1 + ($retry.fault.recoveredOutput.memoryItems | length)))
    and (output_items($retry.fault.recoveredOutput) | length) <=
      $root.effectiveConfiguration.resourceProfile.maxMemoryItemsPerDerivation
    and output_sources_ok($retry; $retry.fault.recoveredOutput)
    and output_anchors_disjoint($retry.fault.recoveredOutput)
    and ($retry.fault.resumeCases[]
      | select(.caseId == "validated-configuration-activation")
      | successful_recovery_transition_ok(
          .signals[0]; .expected; .expectedConsumedSignalIds; .expectedIgnoredSignalIds;
          "validated_configuration_activation";
          $root.repairedRemoteManifest.summaryProvider.providerFingerprint;
          $root.repairedRemoteManifest.configurationFingerprint))
    and ($retry.fault.resumeCases[]
      | select(.caseId == "recorded-provider-healthy-transition")
      | successful_recovery_transition_ok(
          .signals[0]; .expected; .expectedConsumedSignalIds; .expectedIgnoredSignalIds;
          "recorded_provider_healthy_transition";
          $root.effectiveConfiguration.summaryProvider.providerFingerprint;
          $root.effectiveConfiguration.configurationFingerprint))
    and ($retry.fault.resumeCases[]
      | select(.caseId == "user-confirmed-doctor-retry")
      | successful_recovery_transition_ok(
          .signals[0]; .expected; .expectedConsumedSignalIds; .expectedIgnoredSignalIds;
          "user_confirmed_doctor_retry";
          $root.effectiveConfiguration.summaryProvider.providerFingerprint;
          $root.effectiveConfiguration.configurationFingerprint))
    and ($retry.fault.resumeCases[]
      | select(.caseId == "duplicate-and-out-of-order-no-op")
      | [ .signals[].sequence ] == [2, 2, 1]
        and [ .signals[].kind ] == [
          "recorded_provider_healthy_transition",
          "recorded_provider_healthy_transition",
          "user_confirmed_doctor_retry"
        ]
        and .signals[0] == .signals[1]
        and all(.signals[];
          .providerFingerprint ==
            $root.effectiveConfiguration.summaryProvider.providerFingerprint
          and .effectiveManifestFingerprint ==
            $root.effectiveConfiguration.configurationFingerprint)
        and .expectedConsumedSignalIds == [.signals[0].signalId]
        and .expectedIgnoredSignalIds == [
          .signals[1].signalId,
          .signals[2].signalId
        ]
        and .signals[0].sequence == .expected.lastConsumedSequence
        and .signals[2].sequence < .expected.lastConsumedSequence
        and .providerOutcome == "malformed"
        and .expected.budgetBefore == 0
        and .expected.budgetAfterGrant == 1
        and .expected.budgetAfterAttempt == 0
        and .expected.attemptDelta == 1
        and .expected.lastConsumedSequence == 2
        and .expected.ignoredSignalCount == 2
        and .expected.finalState == "retry-exhausted"
        and .expected.durableMemoryCount == 0)
    and $retry.expectedOperationalStatus.reason == "summary_provider_retry_exhausted"
    and $retry.expectedOperationalStatus.safeAction ==
      "repair_summary_provider_or_confirm_retry"
    and $retry.expectedOperationalStatus.pendingCount ==
      $retry.drainCondition.pendingSummaryJobCount;

def redirect_scenario_ok($root; $redirect):
  $redirect.summaryProviderStub.redirectResponse.status == 307
    and $redirect.drainCondition.eventDeliveryState == "committed"
    and $redirect.drainCondition.summaryJobState == "retry-exhausted"
    and $redirect.drainCondition.redirectLocationRequestCount == 0
    and $redirect.drainCondition.redirectLocationPayloadBytesSent == 0
    and $redirect.drainCondition.resentPayloadCount == 0
    and $redirect.securityOracle.consideredRemoteProviderEventCount == 1
    and $redirect.securityOracle.remoteProviderRequestCount == 1
    and $redirect.securityOracle.remoteProviderPayloadCount == 1
    and $redirect.securityOracle.payloadBytesSent ==
      ($redirect.events[0].redactedPayload | utf8bytelength)
    and $redirect.securityOracle.redirectLocationRequestCount ==
      $redirect.drainCondition.redirectLocationRequestCount
    and $redirect.securityOracle.redirectLocationPayloadBytesSent ==
      $redirect.drainCondition.redirectLocationPayloadBytesSent
    and $redirect.securityOracle.resentPayloadCount ==
      $redirect.drainCondition.resentPayloadCount
    and $redirect.expectedOperationalStatus.reason == "provider_redirect_rejected"
    and $redirect.expectedOperationalStatus.safeAction ==
      "activate_non_redirecting_summary_provider"
    and (($redirect.scenarioId == "summary-provider-redirect-rejected"
        and $redirect.fault.redirectRecovery.caseId ==
          "redirect-validated-configuration-activation")
      or ($redirect.scenarioId == "summary-provider-https-to-http-downgrade-rejected"
        and $redirect.fault.redirectRecovery.caseId ==
          "downgrade-validated-configuration-activation"))
    and $redirect.fault.redirectRecovery.signal.targetJobId ==
      $redirect.fault.targetJobId
    and ($redirect.fault.redirectRecovery.expectedConsumedSignalIds ==
      [$redirect.fault.redirectRecovery.signal.signalId])
    and ($redirect.fault.redirectRecovery.expectedIgnoredSignalIds | length) == 0
    and successful_recovery_transition_ok(
      $redirect.fault.redirectRecovery.signal;
      $redirect.fault.redirectRecovery.expected;
      $redirect.fault.redirectRecovery.expectedConsumedSignalIds;
      $redirect.fault.redirectRecovery.expectedIgnoredSignalIds;
      "validated_configuration_activation";
      $root.repairedRemoteManifest.summaryProvider.providerFingerprint;
      $root.repairedRemoteManifest.configurationFingerprint)
    and $redirect.fault.redirectRecovery.oldLocationRequestCountAfterActivation == 0
    and $redirect.fault.redirectRecovery.oldLocationPayloadBytesSentAfterActivation == 0
    and $redirect.fault.redirectRecovery.resentPayloadCountAfterActivation == 0
    and resume_transmission_ok($redirect.fault.redirectRecovery;
      $redirect.providerTransmissionOracle.credentialBytesSent;
      $redirect.providerTransmissionOracle.payloadBytesSent)
    and $redirect.fault.redirectRecovery.expected.finalState == "completed"
    and $redirect.fault.redirectRecovery.expected.durableMemoryCount ==
      (1 + ($redirect.fault.recoveredOutput.memoryItems | length))
    and (output_items($redirect.fault.recoveredOutput) | length) <=
      $root.effectiveConfiguration.resourceProfile.maxMemoryItemsPerDerivation
    and output_sources_ok($redirect; $redirect.fault.recoveredOutput)
    and output_anchors_disjoint($redirect.fault.recoveredOutput);

def redirect_ok($root):
  [ $root.scenarios[] | select(.fault?.kind == "summary_provider_redirect_response") ] as $redirects
  | ([ $redirects[].scenarioId ] | sort) == ([
      "summary-provider-redirect-rejected",
      "summary-provider-https-to-http-downgrade-rejected"
    ] | sort)
    and ($redirects[]
      | select(.scenarioId == "summary-provider-redirect-rejected")
      | .summaryProviderStub.redirectResponse.location ==
        "https://redirect.invalid/v1/summary")
    and ($redirects[]
      | select(.scenarioId == "summary-provider-https-to-http-downgrade-rejected")
      | .summaryProviderStub.redirectResponse.location ==
        "http://summary.stub.invalid/v1/summary")
    and all($redirects[]; . as $redirect | redirect_scenario_ok($root; $redirect));

def output_limit_ok($root):
  fault_scenario($root; "summary_provider_output_limit_exceeded") as $scenario
  | provider_items($scenario) as $items
  | scenario_core_ok($root; $scenario)
    and all($scenario.fault.resumeCases[].signals[];
      .targetJobId == $scenario.fault.targetJobId)
    and all($scenario.fault.resumeCases[]; resume_case_signal_sets_ok(.))
    and all($scenario.fault.resumeCases[];
      resume_transmission_ok(.;
        $scenario.providerTransmissionOracle.credentialBytesSent;
        $scenario.providerTransmissionOracle.payloadBytesSent))
    and $scenario.fault.configuredLimit ==
      $root.effectiveConfiguration.resourceProfile.maxMemoryItemsPerDerivation
    and $scenario.fault.observedResultCount == ($items | length)
    and $scenario.fault.observedResultCount == ($scenario.fault.configuredLimit + 1)
    and $scenario.fault.failureMetadata == {
      "errorCode": "memory_output_limit_exceeded",
      "jobId": "output-limit-job-1",
      "sourceEventIds": [ $scenario.events[].eventId ],
      "observedResultCount": $scenario.fault.observedResultCount,
      "configuredLimit": $scenario.fault.configuredLimit
    }
    and ($scenario.fault.atomicityEvidence as $atomicity
      | $root.lifecycleProfiles[$scenario.lifecycleProfileId] as $profile
      | ($profile | index($atomicity.observationStartMilestone)) as $start
      | ($profile | index($atomicity.observationEndMilestone)) as $end
      | $atomicity.jobId == $scenario.fault.failureMetadata.jobId
        and $atomicity.sourceEventIds == $scenario.fault.failureMetadata.sourceEventIds
        and $atomicity.observedResultCount == $scenario.fault.observedResultCount
        and $atomicity.configuredLimit == $scenario.fault.configuredLimit
        and $atomicity.evidenceSource ==
          "authoritative_writer_receipts_and_durable_observer"
        and $start != null and $end != null and $start < $end
        and $atomicity.writerReceipts == [{
          "receiptId": "output-limit-job-1:writer-attempt-1",
          "jobId": $scenario.fault.failureMetadata.jobId,
          "outcome": "rejected_before_commit",
          "attemptedDerivedItemCount": $scenario.fault.observedResultCount,
          "committedDerivedItemCount": 0,
          "committedMutationCount": 0
        }]
        and [$atomicity.durableObserverSamples[].milestone] == $profile[$start:$end + 1]
        and all($atomicity.durableObserverSamples[];
          .observableDerivedItemCount == 0 and (.forbiddenSentinelObserved | not))
        and $atomicity.committedDerivedBatchCount == 0
        and $atomicity.committedDerivedItemMutationCount == 0
        and $atomicity.maximumObservableDerivedItemCount == 0
        and $atomicity.forbiddenSentinelObservationCount == 0)
    and output_sources_ok($scenario; $scenario.summaryProviderStub)
    and output_anchors_disjoint($scenario.summaryProviderStub)
    and $scenario.drainCondition.summaryCount == 0
    and $scenario.drainCondition.durableMemoryCount == 0
    and $scenario.drainCondition.pendingSummaryJobCount == 1
    and ($scenario.expectedInjectedItems | length) == 0
    and ($scenario.expectedOmissions | length) == 0
    and $scenario.fault.recoveryManifestId ==
      $root.outputLimitRecoveryManifest.manifestId
    and $scenario.fault.resumeCaseInitialSnapshot == {
      "state": "retry-exhausted",
      "budget": 0,
      "lastConsumedSequence": 0
    }
    and [ $scenario.fault.resumeCases[].caseId ] == [
      "validated-larger-limit-activation",
      "unchanged-provider-health-no-op",
      "unchanged-doctor-retry-no-op"
    ]
    and ($scenario.fault.resumeCases[]
      | select(.caseId == "validated-larger-limit-activation")
      | successful_recovery_transition_ok(
          .signals[0]; .expected; .expectedConsumedSignalIds; .expectedIgnoredSignalIds;
          "validated_configuration_activation";
          $root.outputLimitRecoveryManifest.summaryProvider.providerFingerprint;
          $root.outputLimitRecoveryManifest.configurationFingerprint)
        and .expected.finalState == "completed"
        and .expected.durableMemoryCount == ($items | length))
    and ($scenario.fault.resumeCases[]
      | select(.caseId == "unchanged-provider-health-no-op")
      | ignored_noop_transition_ok(.; "recorded_provider_healthy_transition";
          $root.effectiveConfiguration.summaryProvider.providerFingerprint;
          $root.effectiveConfiguration.configurationFingerprint))
    and ($scenario.fault.resumeCases[]
      | select(.caseId == "unchanged-doctor-retry-no-op")
      | ignored_noop_transition_ok(.; "user_confirmed_doctor_retry";
          $root.effectiveConfiguration.summaryProvider.providerFingerprint;
          $root.effectiveConfiguration.configurationFingerprint))
    and $scenario.expectedOperationalStatus.reason == "memory_output_limit_exceeded";

def operational_status_ok($root):
  all($root.scenarios[] | select(has("expectedOperationalStatus"));
    .expectedOperationalStatus.profileId ==
      $root.effectiveConfiguration.resourceProfile.profileId
    and .expectedOperationalStatus.profileVersion ==
      $root.effectiveConfiguration.resourceProfile.version
    and (.expectedInjectedItems | length) == 0
    and .expectedOperationalStatus.pendingCount == 1
    and .drainCondition.pendingSummaryJobCount == 1);

def local_security_ok($root):
  fault_scenario($root; "local_only_remote_provider_ineligible") as $security
  | [ $security.events[].sensitivity ] == ["local_only", "secret"]
    and $root.effectiveConfiguration.summaryProvider.executionLocation == "remote"
    and $security.securityOracle.consideredRemoteProviderEventCount == 2
    and $security.securityOracle.consideredSecretEventCount == 1
    and $security.securityOracle.remoteProviderRequestCount == 0
    and $security.securityOracle.remoteProviderPayloadCount == 0
    and $security.securityOracle.persistedSecretCount == 0
    and $security.securityOracle.injectedSecretCount == 0
    and $security.securityOracle.forbiddenSentinels == [
      "LOCAL_ONLY_SENTINEL",
      "SECRET_SENTINEL"
    ]
    and ($security.securityOracle.forbiddenSentinels | length == (unique | length))
    and all($security.securityOracle.forbiddenSentinels[];
      . as $sentinel
      | any($security.events[]; .text | contains($sentinel)))
    and ($security.securityOracle.sentinelObservedAtRemote | not)
    and ($security.securityOracle.secretObservedInPersistenceOrInjection | not)
    and $security.drainCondition.providerRequestCount == 0
    and $security.drainCondition.providerPayloadCount == 0
    and $security.expectedOperationalStatus.reason ==
      "local_only_remote_provider_ineligible"
    and $security.expectedOperationalStatus.safeAction ==
      "activate_local_summary_provider_or_exclude_local_only";

def private_security_ok($root):
  fault_scenario($root; "private_remote_provider_ineligible") as $security
  | [ $security.events[].sensitivity ] == ["private"]
    and $root.effectiveConfiguration.summaryProvider.executionLocation == "remote"
    and $security.securityOracle.consideredRemoteProviderEventCount == 1
    and $security.securityOracle.consideredPrivateEventCount == 1
    and $security.securityOracle.remoteProviderRequestCount == 0
    and $security.securityOracle.remoteProviderPayloadCount == 0
    and $security.securityOracle.remoteInjectionCount == 0
    and $security.drainCondition.providerRequestCount == 0
    and $security.drainCondition.providerPayloadCount == 0
    and $security.summaryProviderStub.policyRejectedReason ==
      "private_remote_provider_ineligible"
    and ($security.securityOracle.forbiddenSentinels[0] as $sentinel
      | any($security.events[]; .redactedPayload | contains($sentinel)))
    and ($security.securityOracle.sentinelObservedAtRemoteOrInjection | not)
    and $security.expectedOperationalStatus.reason ==
      "private_remote_provider_ineligible"
    and $security.expectedOperationalStatus.safeAction ==
      "activate_local_summary_provider_or_exclude_private";

def mixed_sensitivity_security_ok($root):
  fault_scenario($root; "mixed_sensitivity_allowed_projection") as $scenario
  | provider_items($scenario) as $items
  | [ $scenario.events[].sensitivity ] == ["eligible", "local_only", "private", "secret"]
    and ($items | length) == 1
    and all($items[]; .sensitivity == "eligible" and .sourceEventIds == ["mixed-event-1"])
    and $scenario.securityOracle.consideredRemoteProviderEventCount == 4
    and $scenario.securityOracle.consideredEligibleEventCount == 1
    and $scenario.securityOracle.consideredRestrictedEventCount == 3
    and $scenario.securityOracle.consideredPrivateEventCount == 1
    and $scenario.securityOracle.consideredSecretEventCount == 1
    and $scenario.securityOracle.remoteProviderRequestCount == 1
    and $scenario.securityOracle.remoteProviderPayloadCount == 1
    and $scenario.securityOracle.restrictedPayloadBytesSent == 0
    and all($scenario.securityOracle.forbiddenSentinels[];
      . as $sentinel
      | any($scenario.events[] | select(.sensitivity != "eligible");
        .text | contains($sentinel)))
    and ($scenario.securityOracle.sentinelObservedAtRemote | not);

def scope_security_ok($root):
  ([
    $root.scenarios[]
    | select((.securityOracle?.consideredCrossScopeCandidateCount? // 0) > 0)
  ]) as $matches
  | ($matches | length) == 1
  and ($matches[0] as $scope
  | $scope.sourceRepositoryScope != $scope.targetRepositoryScope
    and ($scope.expectedInjectedItems | length) == 0
    and ($scope.expectedOmissions | length) == 1
    and all($scope.expectedOmissions[]; .reason == "omitted_ineligible")
    and $scope.securityOracle.forbiddenSentinels == ["OTHER_REPO_SENTINEL"]
    and $scope.securityOracle.consideredCrossScopeCandidateCount == 1
    and $scope.securityOracle.incompatibleScopeInjectionCount == 0
    and ($scope.securityOracle.forbiddenSentinels[0] as $sentinel
      | ([
          $scope.expectedOmissions[]
          | select(.fact | contains($sentinel))
        ] | length) == 1
        and any($scope.summaryProviderStub.memoryItems[];
          .body | contains($sentinel)))
    and ($scope.securityOracle.sentinelObservedInInjection | not));

def derived_sensitivity_security_ok($root):
  ([
    $root.scenarios[]
    | select((.securityOracle?.consideredDerivedCandidateCount? // 0) > 0)
  ]) as $matches
  | ($matches | length) == 1
  and ($matches[0] as $scenario
    | provider_items($scenario) as $items
    | $root.effectiveConfiguration.destinationPolicyMap[
        $scenario.targetDestinationClass
      ] as $destinationPolicy
    | $scenario.derivationManifestId == $root.localDerivationManifest.manifestId
    and $root.localDerivationManifest.baseConfigurationFingerprint ==
      $root.effectiveConfiguration.configurationFingerprint
    and $root.localDerivationManifest.summaryProvider.executionLocation == "local"
    and $root.localDerivationManifest.summaryProvider.egressPolicy == "on_device"
    and ($root.lifecycleProfiles[$scenario.lifecycleProfileId] as $milestones
      | ($milestones | index("validated_local_manifest_activated")) as $activation
      | ($milestones | index("local_provider_derived_memory")) as $derivation
      | $activation != null and $derivation != null and $activation < $derivation)
    and $destinationPolicy.executionLocation == "remote"
    and all($scenario.events[]; .sensitivity == "local_only")
    and all($items[]; .sensitivity == "local_only")
    and ($scenario.expectedInjectedItems | length) == 0
    and ($scenario.expectedOmissions | length) == ($items | length)
    and all($scenario.expectedOmissions[];
      .sensitivity == "local_only" and .reason == "omitted_ineligible")
    and $scenario.securityOracle.consideredDerivedCandidateCount == ($items | length)
    and $scenario.securityOracle.remoteInjectionCount == 0
    and $scenario.securityOracle.expectedSensitivity == "local_only"
    and ($scenario.securityOracle.forbiddenSentinels[0] as $sentinel
      | any($scenario.expectedOmissions[]; .fact | contains($sentinel)))
    and ($scenario.securityOracle.sentinelObservedInInjection | not));

. as $root
| ensure(fixture_graph_ok($root); "fixture scenario graph mismatch")
| ensure(host_identity_ok($root); "host-derived identity probe mismatch")
| ensure(provider_transmission_ok($root); "provider transmission oracle mismatch")
| ensure(manifest_contract_ok($root); "effective capability manifest contract mismatch")
| ensure(output_limit_recovery_manifest_ok($root); "output-limit recovery manifest mismatch")
| ensure(before_model_evidence_ok($root); "before-model evidence invariant failed")
| ensure(injection_envelope_ok($root); "injection envelope mismatch")
| ensure(resource_profile_ok($root); "resource profile mismatch")
| ensure(destination_policy_ok($root); "destination policy map mismatch")
| ensure(selection_lifecycle_ok($root); "selection lifecycle boundary mismatch")
| ensure(resource_metrics_ok($root); "resource measurement boundary mismatch")
| ensure(failure_continuation_ok($root); "failure continuation milestone mismatch")
| ensure(pack_degradation_policy_ok($root); "pack degradation policy mismatch")
| ensure(transport_security_ok($root); "off-host transport security mismatch")
| ensure(common_scenarios_ok($root); "common event, count, or per-item provenance invariant failed")
| ensure(bidirectional_ok($root); "bidirectional prompt-flush or content-based derivation invariant failed")
| ensure(spool_ok($root); "spool replay or event-identity conflict invariant failed")
| ensure(retry_ok($root); "retry resume-signal invariant failed")
| ensure(redirect_ok($root); "redirect rejection or repair invariant failed")
| ensure(output_limit_ok($root); "provider output-limit invariant failed")
| ensure(operational_status_ok($root); "operational status invariant failed")
| ensure(local_security_ok($root); "local-only or secret boundary invariant failed")
| ensure(private_security_ok($root); "private egress boundary invariant failed")
| ensure(mixed_sensitivity_security_ok($root); "mixed-sensitivity projection invariant failed")
| ensure(scope_security_ok($root); "cross-scope omission invariant failed")
| ensure(derived_sensitivity_security_ok($root); "derived local-only injection invariant failed")
| true
