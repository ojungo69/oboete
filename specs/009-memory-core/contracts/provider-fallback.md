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
4. Its egress is narrower than or equal to the primary's. `PRESET_CATALOG[preset].egress` is `local`
   or `remote`; a `local` target under a `remote` primary is admitted, a `remote` target under a
   `local` primary is a `ProviderConfigError`. A local selection that could reach the network under
   any failure is not a local selection, and the user who wrote it meant something the configuration
   cannot deliver, so the error belongs at the resolve, not at the send.
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
rule 4 forbids a chain from widening a `local` primary's egress, so that branch stays correct
without naming the chain.

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
   `provider_attempts` increment (`src/observer/reservation.ts:71-82`).
4. `providerCall` and `settleProviderOutcome` for that target, unchanged. Each target keeps its own
   output retries and its own single language-mismatch retry; the chain adds no retry of its own.

`observation_batches.provider_attempts` therefore counts the chain's successful reservations rather
than one per batch. Nothing reads that column as a bound — it is written in
`src/observer/reservation.ts:92` and listed in the insert at `src/worker/batches.ts:602`, and read
nowhere — so a three-target pass does not trip any threshold.

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

A successful target ends the chain and the batch applies its output exactly as it does today.

`language_mismatch` is the one stop that keeps its own reason rather than the most severe of the
attempted ones: `retryOnLanguageMismatch` (`src/worker/observe-batch.ts:501-529`) owns its retry and
its fallback, so a chain that met `provider_exhausted` first and then a second mismatch degrades with
`language_mismatch`. It is the reason of the target that actually answered, which is the more useful
of the two here, and it costs no code.

## When every target fails

No new requeue path. `applyFallback` runs once with a rule-based output, and `outcomeForSource`
(`src/observer/apply.ts:411`) already maps a non-null `fallbackReason` to `{ outcome: 'deferred' }`,
which writes `processing_state = 'waiting'` with `retry_after = sourceRetryAt(now, attempts)` — a
number for every non-partial row, never null, so no source is parked. The sources settle once, so
`processing_attempts` is incremented once for the whole chain: N targets are one attempt at the
batch, which is what CONSTITUTION IV ("accepted information remains available") and US7 scenario 6
require.

The batch's single `degraded_reason` is the most severe reason among the targets actually attempted,
taken with the codebase's existing rule — the first match in `DEGRADED_PRECEDENCE`
(`src/observer/classify.ts:350`, `src/injection/pack.ts:253`). No new reason is minted, so migration
0009 is not needed and the column's CHECK list is untouched. The per-target reasons that the single
column cannot hold go to the observe log, one line per attempted target.

## Diagnostics

- **Observe log**: one `provider attempt` line per target with its position, preset, model and
  outcome reason, then the existing degraded line for the batch. This is the surface that
  "distinguishes each target's fixed failure reason from successful generation" (US7 scenario 6);
  the reasons are codes, never provider response text.
- **`oboete doctor`**: the provider item keeps probing the **primary only**. `providerItem`
  (`src/doctor/provider.ts`) calls `summarizeWithProvider` with a real reservation, so one probe per
  target would spend the daily allowance on diagnostics. The chain is reported statically: each
  target's position, preset, model, admission verdict (admitted / excluded by `cost_policy`), whether
  its credentials are present, and its `provider_usage.exhausted_at` if set.

## What the chain does not do

- **A primary the resolver refuses leaves no chain to try.** `resolveModel` throws on
  `model_required`, `egress_widened` and `chain_without_primary`, and `resolveObserveModel`
  (`src/worker/observe.ts:420-429`) turns that into a run with no model and no targets, so every
  batch is rule-based with `no_provider` and `oboete doctor` is where the user learns why. Absent
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
- It adds no attempt counter. The chain's length is its bound, and each target's internal retries are
  the ones it already had.

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
