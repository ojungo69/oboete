# Provider fallback chain contract

This contract implements T048 together with T037, T038 and T039. It extends
[../007-oboete-m1-alpha/contracts/observer.md](../007-oboete-m1-alpha/contracts/observer.md),
whose sentence "M1 enables exactly one observer preset at a time" this contract retires, and
[generation-privacy.md](generation-privacy.md), whose per-source egress rules it does not relax.

The authority for the behaviour is FR-011 — "Configured model/provider fallback is required after
eligible free-tier/API failures; validate each target's current consent, data eligibility and cost
constraints before sending" — with US7 scenarios 2, 3, 5 and 6, and `CONSTITUTION.md`: "The user
chooses local, free or paid generation and its spending policy; free/local modes MUST NOT switch to
paid generation automatically."

## What changes

`resolveModel(config)` returns one `{ preset, model }` today, and `processBatch` calls it once
(`src/worker/observe-batch.ts:613`). A provider failure sets the session's provider state, degrades
the batch to a rule-based `fallback` and leaves the sources waiting for the next bounded run. US7
scenario 5 requires a second eligible configured target to be tried inside the same pass; scenario 6
requires the accepted source to stay retryable when every target fails, with each target's failure
reason distinguishable.

This contract adds an ordered chain of targets after the primary. Nothing else about batching
changes: one batch, one destination, one request payload, one settlement of its sources.

## Configuration surface

Two keys join `[observer]`, both in `observerSchema` (`z.strictObject`, so an unknown key is
already refused at load):

```toml
[observer]
preset = "workers-ai"
cost_policy = ["free-tier", "local"]

[[observer.fallback]]
preset = "ollama"
model = "qwen2.5:7b"
```

- `fallback` — an ordered array of tables, each `{ preset, model? }` with the same meaning `preset`
  and `model` have for the primary, `preset` drawn from `PRESET_NAMES` (never `none`). At most three
  entries: the chain's bound is its length, so the length is the bound that has to be small.
- `cost_policy` — the cost classes a fallback target may carry, from the four classes
  `PRESET_CATALOG` already assigns (`free-tier`, `local`, `remote`, `own-subscription`). Default
  `["free-tier", "local"]`, which is exactly today's behaviour for every install: no paid class is
  admitted until the user writes one in. This field is the "explicit new choice" of FR-011 and the
  reason US7 scenario 2 holds by construction rather than by a runtime check.

The primary preset is admitted by having been selected; `cost_policy` gates only the chain. A user
who selects `openrouter` as their primary is not asked to also list `remote`.

## Admission

The admitted chain is derived beside `resolveModel`, from the configuration alone, and is what every
later step means by "the chain". `resolveModel` and the derivation report a bad chain the way the
resolver already reports a bad primary: a `ProviderConfigError`, which `resolveObserveModel`
(`src/worker/observe.ts:417-425`) turns into an empty model and therefore a `no_provider` run, and
which `oboete doctor` and `oboete setup` surface as the configuration error it is. A chain mistake
never becomes a capture failure, because capture reaches neither the resolver nor `consentTuple`.

A target is admitted when all of the following hold:

1. The primary is a real preset. `preset = "none"` with a non-empty `fallback` is a
   `ProviderConfigError`: a chain with no primary has no selected destination to narrow, and
   silently ignoring the entries would hide a destination the user wrote down.
2. Its `preset` resolves a non-empty model, by the same rule as the primary — `[observer] model` is
   the primary's only, so a chain entry on a preset with an empty `defaultModel` (`ollama`) must
   carry its own `model`. A missing model is a `ProviderConfigError` with code `model_required`,
   naming the chain position.
3. Its `(preset, resolvedModel)` pair has not already appeared, counting the primary as position
   zero. A later duplicate is dropped, not refused: the same preset with two different models is two
   targets, and the same pair twice is one.
4. Its egress is narrower than or equal to the primary's. A `local` target under a `remote` primary
   is admitted; a `remote` target under any primary that is not itself `remote` is a
   `ProviderConfigError`. The test is written against `remote` rather than against `local` because
   `ProviderPreset['egress']` also has `none`: no preset carries it today, and a target that could
   reach the network under a primary that cannot is the same mistake whichever of the two narrower
   classes the primary has. A local selection that could reach the network under any failure is not
   a local selection, and the user who wrote it meant something the configuration cannot deliver, so
   the error belongs at the resolve, not at the send.
5. Its `costClass` is in `cost_policy`. A target that fails only this test is **skipped, not
   refused**: it stays in the file, contributes nothing, and is reported by `oboete doctor` as
   excluded by the policy. The asymmetry with rule 4 is deliberate — the cost policy is a live
   switch the user flips to admit a target they have already written down, while widening egress is
   never what the user meant.

Credentials are not an admission test. An environment variable can appear between one batch and the
next, so a target whose credentials are absent is admitted and fails its attempt with `no_provider`
— `providerConfigured` in `src/observer/llm.ts:307` is false when `credentials.present` is false,
and `summarizeWithProvider` answers `failure('no_provider', 0, …)` without a request — which
advances the chain. `oboete doctor` reports the absence statically so the user does not discover it
only from a degraded batch.

### The destination label, and per-attempt eligibility

`observation_batches.destination` is unchanged, and it stays the primary's. It is an authorization
label re-validated per pass by `reconcilePendingDestinations(db, token, now, primaryEgress)`
(`src/worker/batches.ts:491`), whose comment states that reclaiming an old attempt never reuses its
destination authorization for a different preset.

**A target is attempted only when its egress is narrower than or equal to the batch's `destination`
label**: `remote_observer` admits a local or a remote target, `local_observer` admits a local target
only. This is T048's "per-attempt source eligibility", and under rule 4 it is satisfied by every
admitted target of a pending batch — it is written as its own rule because it is the invariant that
keeps the label honest, not a consequence of the configuration. The label keeps meaning "at most
this far", the reconcile call keeps passing the primary's egress, and nothing reads the label as a
record of where a batch actually went.

## Consent coverage

`consentTuple` covers only the primary today (`src/config.ts:396`) and `consentHash` binds exactly
its five fields. A chain outside the tuple would let stored consent authorize a destination the user
never saw, which FR-011 and US7 scenario 4 forbid.

The tuple gains one field, `chain`: for each admitted target in order, its `preset`, `host`,
`credentialSource`, `costClass` and `egressClasses` — the same five facts the primary contributes.
`consentHash` appends the chain to its hashed array **only when the admitted chain is non-empty**,
so an install with no chain — every install that exists today, and every install that keeps the
default `cost_policy` with no `fallback` entries — hashes exactly as it does now and is not asked to
re-consent on upgrade. Because the field carries the *admitted* chain, a `cost_policy` edit that
admits a new target changes the hash by construction, and one that admits nothing changes nothing:
there is no new destination to consent to.

`consentTuple` takes the admitted chain as an argument rather than deriving it, so it cannot throw:
`src/worker/observe.ts:176` recomputes the hash on every pass, and a configuration error there must
degrade the run, not crash it.

`consentMatches` is unchanged. Its no-stored-record branch already refuses any `remote` egress, and
rule 4 forbids a chain from widening a non-`remote` primary's egress, so that branch stays correct
without naming the chain.

A target's displayed **sensitivity classes are the primary's**, not its own preset's capability: the
chain never re-batches, so a local target under a remote primary receives the `remote_observer`
batch and sees only what that destination may carry. The hashed `egressClasses` stay the target's own
(they describe the destination, which is what consent binds); the displayed line describes what is
sent, and overstating it would make a user refuse a target that never receives the material.

`setup` displays the chain with the primary — one line per target with its host and cost class — so
the consent the user accepts is the consent the hash binds.

## The attempt sequence

The loop wraps the existing call and settlement (`src/worker/observe-batch.ts:613-628`), over the
primary followed by each admitted target. Everything before the loop happens once: privacy
revalidation, `reconcilePendingDestinations`, the nearby and checkpoint context,
`buildObserverRequest`, the final detector check on the request, and `markRequest`. One batch
produces one request payload, and every target receives that same payload.

For each target, in order:

1. `deps.shouldStop()` — a stop sentinel or a due exit ends the pass between targets, as it does
   today inside `providerCall`'s consent boundary.
2. `currentConsent()` — the same closure the primary uses, so a consent, privacy-stamp, checkpoint
   or nearby change between targets stops the send exactly as it stops a retry today.
3. `reserveAttempt(db, { preset, capped: PRESET_CATALOG[preset].capped, … })` for *that* target's
   preset. The daily allowance is summed over capped presets and `provider_usage.exhausted_at` is
   per-preset, so the reservation is what makes "shared quota versus per-target failure" (T048's
   phrase) come out right without any new accounting. A refusal writes nothing: `reserveAttempt`
   returns `daily_cap` or `provider_exhausted` before `recordProviderAttempt` and before the
   `provider_attempts` increment (`src/observer/reservation.ts`).

   **Every target reserves, capped or not.** `capped: false` skips the shared-cap check and nothing
   else, because the reservation is also the fence: it is what sets `state = 'running'` and
   restamps `claimed_at`, which is what gives an in-flight call the 120 s `reclaimStale` grants
   before another worker may take the batch (`RECLAIM_AFTER_MS` in `src/worker/batches.ts`). A
   target that skipped it would run with the batch still `pending`, which `adoptPendingBatches`
   takes over at once and without any wait — so an `own-subscription` target would pay a second
   time for the same payload whenever a worker died with its child process in flight. `claimed_at`
   is therefore restamped at the reservation and not only at batch creation; a batch created
   minutes earlier would otherwise be reclaimable the instant it started running.
4. `providerCall` and `settleProviderOutcome` for that target, unchanged. Each target keeps its own
   output retries and its own single language-mismatch retry; the chain adds no retry of its own.

`observation_batches.provider_attempts` therefore counts the chain's successful reservations rather
than one per batch. Nothing reads that column as a bound — it is written in
`src/observer/reservation.ts:92` and listed in the insert at `src/worker/batches.ts:602`, and read
nowhere — so a three-target pass does not trip any threshold.

**Identity.** A target is `(preset, model)`, except `agent-cli`, which is `(preset, agent_cli)`:
`summarizeWithAgentCli` reads `[observer] model` only as a non-empty gate and `runAgentCli` is never
given it, so two entries on the same command line tool invoke the identical call. Admitting them as
two targets would let one advancing failure pay that subscription twice for one payload, which is
the shape US7 scenario 2 forbids. The other way to close it — sending the model to the CLI — widens
what oboete asks of the subscription and is issue #241.

## Advance and stop

The chain exists for failures to obtain an answer, not for the quality of an answer that arrived.
Every `FailureReason` therefore falls into one of two cases:

| reason | chain | why |
|---|---|---|
| `provider_exhausted` | advance | per-preset `exhausted_at`; the next target is a different preset |
| `daily_cap` | advance | every remaining capped target then refuses at its own reservation, with no request and no write, so each one logs its own `daily_cap` instead of vanishing; `ollama` and `agent-cli` are unaffected by the cap |
| `provider_paid` | advance | this preset would bill; another may not |
| `auth_failed` | advance | this preset's credentials were rejected |
| `no_provider` | advance | this target's credentials are absent or its model is empty; no request was made |
| `unreachable`, `timeout` | advance | no answer from this host |
| `model_alias` | advance | this target's model is not the model it claims |
| `consent_changed` | stop | consent no longer authorizes any destination; a later target is not more authorized than this one |
| `unusable_output`, `language_mismatch` | stop | the request reached a provider, was answered, and spent that target's allowance; both reasons already own their retries, and spending a second allowance on the same payload is the paid-by-accident shape US7 scenario 2 forbids |

The column is read from **the reason the target settles with**, not from anything an earlier attempt
of that target produced. A target whose first attempt answered unusably and whose retry then failed
in transit settles with `timeout` or `unreachable` and therefore advances: the row's reason — "no
answer from this host" — is what happened, and a later target plainly can improve on a dropped
connection. Stopping there instead would strand the batch on a transport error while an admitted
local target sat unused, which is what US7 scenario 5 asks the chain to prevent. The evidence the
stop rule is about is two unusable answers, and that is exactly the case
`summarizeWithProvider` reports as `unusable_output`.

A successful target ends the chain and the batch applies its output exactly as it does today.

**The reason a stop ended the chain on outranks the precedence order.** A chain that met
`auth_failed` and then stopped on `consent_changed` degrades with `consent_changed`, even though
`auth_failed` is the more severe of the two in `DEGRADED_PRECEDENCE`: consent is the thing the user
has to act on, and sending them to fix a credential instead would be the wrong instruction. Only
when the chain ran out of targets does the batch keep the most severe reason it met.
`language_mismatch` reaches this rule from a different direction — `retryOnLanguageMismatch`
(`src/worker/observe-batch.ts:501-529`) owns its retry and its own fallback — but the outcome is the
same: the target that actually answered is the one the batch names, and its attempt line is recorded
where it settles rather than in the loop.

## When every target fails

No new requeue path. `applyFallback` runs once with a rule-based output, and `outcomeForSource`
(`src/observer/apply.ts:411`) already maps a non-null `fallbackReason` to `{ outcome: 'deferred' }`,
which writes `processing_state = 'waiting'` with `retry_after = sourceRetryAt(now, attempts)` — a
number for every non-partial row, never null, so no source is parked. The sources settle once, so
`processing_attempts` is incremented once for the whole chain: N targets are one attempt at the
batch, which is what CONSTITUTION IV ("accepted information remains available") and US7 scenario 6
require.

The batch's single `degraded_reason` is the most severe reason among the targets actually attempted,
taken with the codebase's existing rule — the first match in `DEGRADED_PRECEDENCE`, named once as
`mostSevereReason` (`src/observer/classify.ts`). `src/injection/pack.ts` keeps its own copy of the
expression on purpose: its local `DegradedReason` is wider (it adds `index_unavailable`,
`summary_pending`, `window_unknown` and `empty`), so sharing the helper would either narrow the pack
or widen the batch column's closed reason set. No new reason is minted, so migration
0009 is not needed and the column's CHECK list is untouched. The per-target reasons that the single
column cannot hold go to the observe log, one line per attempted target.

## Diagnostics

- **Observe log**: one `provider attempt` line per target with its position, preset, model and
  outcome reason, then one `batch` line. That batch line is written for every batch the pass
  reached, with one exception — a pass that stops between targets writes its attempt lines and no
  batch line — and it is written even when the pass then fails: `applyObservations` commits the row
  before the checkpoint that follows it, so a line left out would leave nothing at all in the log
  for a batch the database says is applied. It carries the *batch's* `state` and `reason`, and
  separately the *pass's* `error` and `pass` — kept apart because a batch that settled on a reason
  of its own would otherwise hide the code of whatever failed later. `pass` takes both conditions at
  once — a **storage** failure **out of the checkpoint**, which is the one that ends the run — and
  `error` is everything else: anything out of `processBatch`, storage or not, and a non-storage
  checkpoint failure the worker carries on past. The attempt lines are written with the swallowing
  writer and the batch line with the throwing one: an unwritable log is a storage failure worth exit
  3, but a pass that stops must still clear the worker-stop sentinel, and one attempt append must
  not take the batch line with it. This is the surface that
  "distinguishes each target's fixed failure reason from successful generation" (US7 scenario 6);
  the reasons are codes, never provider response text. The line's `position` is the attempt order
  with the primary at 0, while `fallback:N` in `oboete doctor` numbers the *configuration's* entries
  — an entry the policy excludes has a doctor number and no attempt position — so `model` is what
  identifies one target across the two surfaces.
- **`oboete doctor`**: the provider item keeps probing the **primary only**. `providerItem`
  (`src/doctor/provider.ts`) calls `summarizeWithProvider` with a real reservation, so one probe per
  target would spend the daily allowance on diagnostics. The chain is reported statically: each
  target's position, preset, model, admission verdict (admitted / excluded by `cost_policy` or
  covered by a nearer target — two verdicts, not one sentence, because adding the cost class cannot
  make a duplicate runnable and `admittedChain` is what decides which of the two it was), whether
  its credentials are present, and its
  `provider_usage.exhausted_at` if set. Two verdicts it does not overstate: an `agent-cli` target is
  `unverified` rather than ready, because `readCredentials` calls an agent login present and only
  `setup` checks it; and a **capped** target is a warning once the shared allowance is spent, which
  `allowanceItem` reports only when the *primary* is capped. A **Workers AI** target adds a third:
  its model is checked against the cached catalog, because `catalogItems` validates the primary's
  model and returns nothing at all when another preset is primary. A current list for this account
  that omits the model makes the target a warning naming it — the attempt answers `unreachable`,
  since `classifyApiError` has no row for the status an unserved model returns and `model_alias` is
  a *successful* call that named another model — and the target is reported the way it was
  otherwise in every other cache state, because a missing, foreign-account or stale list is one the
  worker replaces and may not refuse anything. The catalog is fetched only for a `workers-ai`
  **primary** today, so a chain-only Workers AI target has no list to check against at all
  (issue #250); the check is silent there rather than printing a recovery that would never come
  true. The two halves of that verdict come
  from different places on purpose: a target's `exhausted_at` is read from its own
  `provider_usage` row (`presetExhaustedAt`), while the spent-allowance warning comes from the
  shared call count alone (`usageEstimate`). A day-wide exhaustion flag would let one preset's
  stamp answer for a preset that reported nothing, which is why `usageEstimate` carries no such
  field.

  Consent is not part of a target's verdict — it is one hash over the primary and the whole chain,
  so a record that no longer matches stops every target and no per-target verdict below it means
  anything. The chain report therefore collapses the way it does on a resolver refusal: one
  **degraded** `fallback` item naming how many entries are configured and recovering with
  `oboete setup --accept-egress`, in place of the per-entry items. Being degraded, it also moves
  `oboete doctor`'s exit to 1, which a report that called an unreachable target ready did not.

  The `provider` item carries the same mismatch, which is the half that matters when no chain is
  configured — the schema's default — because `fallbackItems` returns nothing then. It is read
  where the worker reads it: `configuredProvider` tests consent after the preset, the model and the
  credentials, the order `initialProviderFailure` uses, and answers before any probe, since a probe
  under a stale record can only come back `consent_changed` and would spend a reservation saying so.
  There is no `consent` item in the report, so nothing may point at one: a mismatch is named where
  it is noticed. `oboete setup` is the surface that displays the tuple and takes the acceptance
  (FR-022).

## What the chain does not do

- **A refused configuration never reaches the attempt sequence.** `processBatch` answers it where it
  answers `preset = "none"`: an empty model from `resolveObserveModel` means no target exists, so the
  batch falls back with `no_provider` before a request is built. Nothing else would be true — the
  sequence's step 2 would stop such a batch with `consent_changed` under a stale record and send the
  user to accept an egress that leaves the resolver error in place, step 3's reservation is never
  reached because `summarizeWithProvider` refuses first, and an attempt line naming an empty model
  would record a target that does not exist. The catalog refresh is skipped for the same reason: a
  run that cannot reach a provider has no use for the account's model list.
- **A primary the resolver refuses leaves no chain to try.** `resolveModel` throws on
  `model_required`, `egress_widened` and `chain_without_primary`, and `resolveObserveModel`
  (`src/worker/observe.ts:420-429`) turns that into a run with no model and no targets, so every
  batch is rule-based with `no_provider` and `oboete doctor` is where the user learns why: the
  `provider` item carries that sentence whether or not a chain is configured, and the chain report
  replaces its per-target items with the same one rather than calling any target ready. Absent
  *credentials* are not that case: the destination label comes from the primary's egress class alone
  (`destinationFor` in `src/worker/batches.ts:400-415` reads `PRESET_CATALOG[preset].egress`, never
  the secret), so the loop is reached and the uncredentialed primary is just the first target to
  answer `no_provider`. A workers-ai primary with no `OBOETE_CF_API_TOKEN` and a working `ollama`
  target therefore applies the local target's output (Verification 15). `initialProviderReason` still
  decides the reason a batch already stamped `fallback` records, which is the per-row privacy split
  below and not a property of the chain.
- It never re-batches. Under a remote primary, `local_only` and `private` rows go to a rule-based
  `fallback` batch at batching time, as generation-privacy.md specifies; a local target later in the
  chain does not make them eligible. Sending the remote batch's payload to a local target is
  narrowing, and re-selecting rows for a target would be a different batch identity.
- It never changes the payload between targets. Every target receives the payload the final detector
  check approved.
- It adds no attempt counter. The chain's length is its bound, and each target's internal retries
  are the ones it already had — but the *reservations* one batch can take multiply with it. One
  target takes up to four (`summarizeWithProvider`'s own `while (attempts < 2)`, once more through
  the language retry), so a four-target chain can take up to sixteen where a single preset took
  four. `DAILY_CAP` still bounds the day, because every target passes its own `reserveAttempt`
  before any request. What the length does divide is `SESSION_END_RESERVE`: the ten calls held back
  for session-end batches cover fewer such batches when several *capped* targets are configured.
  The default `cost_policy` admits only one capped preset (`workers-ai`; `ollama` and `agent-cli`
  are uncapped and the other remote presets are outside the default policy), so this needs a
  deliberate `cost_policy = [… "remote"]` to reach.
- It does not move the chain's length bound out of the configuration schema. A fourth
  `[[observer.fallback]]` table is a `ConfigError` from `loadConfig` like any other malformed
  configuration, with the same consequence capture already has for one (metadata-only events until
  it is corrected). The three chain mistakes the *resolver* owns — a missing model, a widening
  egress, a chain with no primary — are the ones that degrade only the observer.

## Verification

1. `consentHash` for a fixture configuration with no `fallback` entries equals the **literal** digest
   that formula produces on `main` (the constant is written into the test, not recomputed by calling
   the new code), and the same fixture with one admitted target hashes differently. Both directions
   pinned: the second half is what stops the first from passing by construction.
2. A `local` primary with a `remote` entry in `fallback` is a `ProviderConfigError` from the resolver,
   before any send; `resolveObserveModel` turns it into a `no_provider` run rather than a crash.
3. A `remote` primary with a `local` entry is admitted, and its `reconcilePendingDestinations` call
   still passes the primary's egress.
4. With the default `cost_policy` and a `remote` target listed, the chain makes **zero** requests to
   that target's host; with `remote` added to `cost_policy` and nothing else changed, it makes
   exactly one. Same fixture, one key apart.
5. A chain entry on `ollama` with no `model` is a `model_required` error naming its position; the
   same entry with a model is admitted. `preset = "none"` with any `fallback` entry is refused.
6. `(preset, model)` deduplication: a fallback entry equal to the primary is dropped; the same preset
   with a different model is kept as its own target.
7. `provider_exhausted` on the primary advances to the next target, and the exhausted preset is
   skipped on the **next** batch too (`exhausted_at` is per-preset and outlives the pass).
8. `daily_cap` on `workers-ai` advances to `ollama`, which is attempted; a `nim` target in the same
   chain is refused at its own reservation with `daily_cap` and its host receives no request.
9. A target whose credential variable is unset is attempted, answers `no_provider` without a request,
   and the chain advances past it.
10. `consent_changed` between two targets stops the chain: the second target's host receives no
    request.
11. `unusable_output` from the primary stops the chain, and the batch degrades with that reason.
12. All targets failing with different reasons: one `applyFallback`, `degraded_reason` is the most
    severe by `DEGRADED_PRECEDENCE`, every source is `waiting` with a non-null `retry_after`, and
    `processing_attempts` is incremented by exactly one while `provider_attempts` counts the
    reservations the chain actually took.
13. A target that succeeds after two failures applies its output normally: the batch is `applied`,
    the sources are `processed`, and the observe log carries one line per failed target.
14. `oboete doctor` with a three-target chain makes exactly one provider request, and lists every
    target's admission verdict and credential presence.
15. A `workers-ai` primary with no credentials and an `ollama` target applies the ollama output: the
    primary's host receives no request, ollama receives exactly one, and the batch is `applied` on
    the `remote_observer` destination the primary's egress chose.
16. A chain that fails `auth_failed` and then stops on `consent_changed` degrades with
    `consent_changed`, and both attempt lines are in the observe log.
17. A target whose answer is refused for its language has its own `provider attempt` line, at its
    own position, with `language_mismatch`.
18. `oboete setup --remove`, a bare `oboete setup` and `--provider none` all succeed while a chain
    the configuration cannot use sits in the file; only a `--provider` that narrows egress under an
    admitted chain is refused, and it writes nothing. Every other chain error is **reported and the
    run continues**, the way a missing credential is (contracts/cli.md: setup prints the steps
    "instead of failing"): a `--provider` over an entry with no model succeeds, writes the
    destination and names the entry, because the entry is the file's defect rather than the
    destination's doing — the selected preset does not run until it is corrected, which is what the
    report says. **A bare `oboete setup` names it too**, because the same entry takes the stored
    primary down and nothing else in the report mentions it: the consent display lists the targets
    of an *admitted* chain, and an unusable chain has none. `--remove` is the one run that does not
    look, because it is the recovery path. `admittedChain` returns at the first entry it refuses, so
    entries after it are unexamined and the report says that too rather than restating the admission
    rules.
19. Two identical `[[observer.fallback]]` entries: `fallback:1` is healthy and `fallback:2` says a
    nearer target already covers it. `preset = "none"` with an entry reports the missing primary
    rather than "fallback target 0".
