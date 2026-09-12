# Injection validation snapshot — measured C3 correction

The C2 none-provider 1,051-event replay exceeded the 300 ms ordinary hook bound. An isolated
hook over a backup of that synthetic database took 483.5 ms inside the process, including 36 Git
invocations totaling 153.9 ms. The same verified source root was resolved repeatedly by each
source-specific detector and again for the whole pack. Do not relax the hook budget or source policy.

The existing assembly already obtains one privacy snapshot for all kept references before any
asynchronous detection, then independently recomputes its guard immediately before writing the
injection. Reuse that initial snapshot's source policies, full stored fields and base detector for
all checks within that one pack. The reference must be present in that snapshot; an unknown
reference fails closed. A base-only check uses only the captured base policy. A prepared snapshot
is local to one assembly and is never persisted or shared with another pack or hook.

Keep the final fresh `injectionPrivacyValid` check, the transactional work-selection check, and
Grok's later emission/merge validation. Those calls receive no cached state. A policy, credential,
source label/body/proof, binding, work selection or directory generation that changed after the
initial snapshot cancels the prepared pack. Deferred merging still uses its independent current
validation. Source-specific policies already include home/current-repository rules plus origin
rules; their checks need not repeat the identical text under the narrower base policy.

Verify the actual kept-reference text against the same snapshot whose digest is checked at the
end, including races during async detection and removed/replaced source roots. Record before/after
Git-call and wall-time measurements on the same synthetic workload. Required checks are the
existing work-reader, injection, deferred and Pi privacy/race tests plus the real CLI reproduction.
Worker RSS is a separate measured issue; this change does not claim to resolve it.

## Worker RSS: disable unused library profiling

The C2 uninstrumented replay reached 188.1 MiB. Bounded heap profiles attribute 77.6% of retained
fallback-run samples to Secretlint's global performance marks/measures, rather than source text.
Its installed 13.0.5 API and [versioned upstream README](https://github.com/secretlint/secretlint/blob/v13.0.5/packages/@secretlint/profiler/README.md)
document `secretLintProfiler.setEnabled(false)` for library callers. Set it once in the detector
module before any lint call, in both main processes and detector threads. All rules and revalidation
remain enabled. Declare the already-installed `@secretlint/profiler@13.0.5` directly alongside the
other bundled Secretlint build dependencies, because the engine now imports its shared instance.
The lockfile must retain the same dependency version/integrity. A repeated-detector check must
still redact secrets while retaining zero profiler entries; repeat the uninstrumented replay for RSS.

## Two packs in one Codex response

A reproduced race changed credentials while the second pack was being validated: the earlier
start pack was still printed and had already been marked delivered. Keep its items planned until
the combined response is ready. Pass its planned memory IDs to prompt selection only to suppress
duplicates in that same response; this is not a delivered-memory receipt. After the second await,
freshly validate the earlier pack's work/privacy guard, omit it if stale, then confirm only the
surviving output. Reuse the existing planned-item omission helper for cancellation. Other agent
paths keep their single-pack or deferred delivery protocol.

If the second await throws, cancel the first plan best-effort before returning the original hook
failure. Mark caller-cancelled start packs `omitted/not_delivered` and permit their retry; ordinary
empty/unsupported omissions still count as attempts. A cancelled plan is neither a delivered item
nor proof that the epoch received its start pack.
