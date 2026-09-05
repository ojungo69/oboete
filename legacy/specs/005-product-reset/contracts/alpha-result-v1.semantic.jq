def ensure($condition; $message):
  if $condition then . else error($message) end;

def drain_ok:
  (.drain.timedOut == (.drain.status == "timed_out"));

def milestones_ok:
  . as $result
  | ([$result.milestones[].name] | length == (unique | length))
  and ([$result.milestones[].monotonicMs] as $times
    | all(range(1; $times | length); $times[.] > $times[. - 1]))
  and (if ($result.drain.timedOut
      or $result.disposition.state == "unsupported"
      or $result.disposition.state == "not_run")
    then true
    else any($result.milestones[]; .name == $result.drain.terminalMilestone)
    end);

def late_injection_ok:
  . as $result
  | ([ $result.milestones[] | select(.name == "target_model_request_dispatched") ]
      | if length == 1 then .[0] else null end) as $dispatch
  | ([ $result.milestones[] | select(.name == "target_injection_acknowledged") ]
      | if length == 1 then .[0] else null end) as $injection
  | if $result.injectionBeforeModel == null
    then true
    elif $dispatch == null or $injection == null
    then false
    else $result.injectionBeforeModel ==
      (([$result.milestones[].name] | index($injection.name)) <
        ([$result.milestones[].name] | index($dispatch.name))
        and $injection.monotonicMs < $dispatch.monotonicMs)
    end;

def counts_ok:
  .counts.tracedCandidates + .counts.deadlineUnprocessed == .counts.inputCandidates
  and .counts.admittedCandidates <= .counts.tracedCandidates
  and .counts.selectedItems <= .counts.admittedCandidates
  and .counts.selectedItems == (.injectedItems | length)
  and .counts.summaryCount <= .counts.durableMemoryCount
  and (if .securityEvidence.remoteProviderRequestCount > 0
    then .securityDenominators.consideredRemoteProviderEventCount > 0
    else true
    end)
  and .securityEvidence.remoteProviderPayloadCount <=
    .securityEvidence.remoteProviderRequestCount
  and (if .securityEvidence.remoteProviderRequestCount == 0
    then .securityEvidence.credentialBytesSent == 0
    else true
    end)
  and (if .securityEvidence.remoteProviderPayloadCount > 0
    then .securityEvidence.payloadBytesSent > 0
    else true
    end)
  and .securityEvidence.restrictedPayloadBytesSent == 0
  and (if .securityEvidence.redirectLocationRequestCount == 0
    then .securityEvidence.redirectLocationPayloadBytesSent == 0
    else true
    end)
  and .attemptedRenderedBytes >= .renderedBytes
  and .attemptedInjectedTokens >= .injectedTokens
  and ((.finalRenderEvidence == null) == (.packId == null))
  and (if .finalRenderEvidence == null
    then .renderedBytes == 0 and .injectedTokens == 0
    else .renderedBytes > 0 and .injectedTokens > 0
    end)
  and .counts.committed <= .counts.captured
  and .counts.lost <= .counts.captured
  and (if (.counts.deadlineUnprocessed > 0
      or (.drain.timedOut
        and ([.milestones[].name] | index("target_selection_finished")) == null))
    then .counts.selectedItems == 0
      and (.injectedItems | length) == 0
      and .renderedBytes == 0
    else true
    end);

def process_samples_ok:
  ([.processSamples[].monotonicMs] as $times
    | all(range(1; $times | length); $times[.] >= $times[. - 1]));

def host_identity_ok:
  .hostIdentityEvidence == null
  or (.hostIdentityEvidence.consideredClaimCount ==
      (.hostIdentityEvidence.decisions | length)
    and .hostIdentityEvidence.claimAuthorizedPersistenceCount == 0
    and .hostIdentityEvidence.claimAuthorizedInjectionCount == 0
    and all(.hostIdentityEvidence.decisions[]; .authorityAccepted | not));

. as $result
| ensure(drain_ok; "drain/disposition mismatch")
| ensure(milestones_ok; "milestone order mismatch")
| ensure(late_injection_ok; "late-injection negative mismatch")
| ensure(counts_ok; "count relationship mismatch")
| ensure(process_samples_ok; "process sample order mismatch")
| ensure(host_identity_ok; "host identity evidence mismatch")
| true
