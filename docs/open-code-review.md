# OpenCodeReview in GitHub Actions

`.github/workflows/open-code-review.yml` runs the official
[Alibaba OpenCodeReview action](https://github.com/alibaba/open-code-review/blob/486022daaf14f7142275eddb9b3cacc3cc5dadfa/action.yml).
The action is pinned to a commit, and its CLI is pinned to version 1.12.9.
The job sets `OCR_NO_UPDATE=1`: otherwise the CLI's first call starts a background
`npm i -g` of the newest release, which replaced the pinned one while the action was
still calling it ("Cannot find module"): 4 of the 9 runs that failed up to 2026-09-28.
It posts Japanese inline findings and updates one summary comment on the PR.
It supplements the existing Rust CI and SonarCloud checks; it does not approve or merge PRs.

## Provider setup

The primary is DeepSeek V4.1 Flash through OpenRouter (`deepseek/deepseek-v4.1-flash`),
served by DeepSeek's own API with the owner's DeepSeek key, which the owner
registered in OpenRouter as a BYOK key (owner, 2026-09-29: "if OpenCodeReview is
slow, use DeepSeek from OpenRouter, where I registered BYOK"). The fallback is the
same model on NVIDIA NIM (`deepseek-ai/deepseek-v4.1-flash`), which costs nothing.
Both use `high` reasoning effort (the owner lowered it from `max` on 2026-09-28).
OCR's separate review effort is at its maximum, `high` (three review rounds); OCR
does not accept `max` for that setting.

- **Why not NIM first.** Measured locally with OCR 1.12.9 on #225's merged change
  (4bbe0fa..82ec6be, seven files in three groups, the workflow's settings), NIM
  took 18 minutes and DeepSeek's API 9 minutes 16 seconds. A call took 18 seconds
  on NIM on average, 4 on DeepSeek's API (60 and 134 calls). NIM's gateway also
  answers 504 to a call still running at about 300 seconds (below).
- **Routing.** `OCR_LLM_EXTRA_BODY` is `{"provider":{"only":["deepseek"],"allow_fallbacks":false}}`:
  OpenRouter tries BYOK endpoints first, and this keeps it off the model's other
  providers, which would spend OpenRouter credits. It does not keep a call off
  OpenRouter's own DeepSeek endpoint when the BYOK key fails: OpenRouter tries its
  shared capacity after the BYOK keys ([BYOK](https://openrouter.ai/docs/guides/overview/auth/byok)).
  The BYOK key's option "Never use shared capacity for models this key applies to"
  closes that route; a failed key then fails the primary, and the fallback runs on
  NIM. A response shows the route: `"provider": "DeepSeek"` and `"is_byok": true`
  in its usage.
- **Cost.** DeepSeek bills the owner's account at its price. OpenRouter charges 5%
  of the OpenRouter price for BYOK calls beyond a free monthly allowance
  ([BYOK](https://openrouter.ai/docs/guides/overview/auth/byok), read 2026-09-29), and
  charged nothing on these runs (`"cost": 0`). The measured seven-file review
  cost USD 0.10: the key's `byok_usage` rose by that much. It used 4,450,691
  tokens, 95% of them cached input, which DeepSeek prices at USD 0.003 per million
  (USD 0.15 per million uncached, USD 0.60 per million output, OpenRouter's listing).
  `byok_usage` in `GET https://openrouter.ai/api/v1/key` shows what reviews spend.
- **Key.** An OpenRouter key can spend through every BYOK provider on its account,
  including ones billed after use. Give the workflow its own key, with a limit
  that counts BYOK usage (`include_byok_in_limit`), so a leaked key cannot spend
  past it. The model reads only files inside the repository: OCR's `file_read`
  refuses paths outside it and `code_search` refuses `..`
  ([filereader.go](https://github.com/alibaba/open-code-review/blob/bccbc15/internal/tool/filereader.go)).

Until 2026-09-29 the primary was the same model on NIM, with `z-ai/glm-5.3` as the
fallback. Its first two reviews on main, of PR #231 at heads 2eead29 and 0129e12
(runs 36483093911 and 36484506317), finished in 11 minutes 42 seconds and 8 minutes
31 seconds with 120,831 and 44,327 tokens, no retry, no finding and no fallback. Before that GLM was the primary
and `moonshotai/kimi-k3` the fallback. Kimi returned no token in any of its eight
fallback attempts on 2026-09-28, each stopped at the 15-minute deadline (runs
36394141175 to 36456002254). GLM stopped at that deadline in the six of those runs
where it started (the other two failed at launch, before #217). For DeepSeek, NIM
accepts `reasoning_effort` `none`, `low`, `high` and `max`, and rejects `medium`
with a 400 (measured 2026-09-29).

In the repository's **Settings > Secrets and variables > Actions**, configure:

| Kind | Name | Value |
| --- | --- | --- |
| Secret | `OCR_LLM_URL` | `https://openrouter.ai/api/v1/chat/completions` |
| Secret | `OCR_LLM_AUTH_TOKEN` | OpenRouter API key. |
| Secret | `OCR_LLM_FALLBACK_URL` | `https://integrate.api.nvidia.com/v1/chat/completions`. Set it with the next one, or neither. Unset: the fallback uses `OCR_LLM_URL` without `OCR_LLM_EXTRA_BODY`, so on OpenRouter it is not pinned to the BYOK key and may spend credits. |
| Secret | `OCR_LLM_FALLBACK_AUTH_TOKEN` | NVIDIA NIM API key. Unset: the fallback uses `OCR_LLM_AUTH_TOKEN`. |
| Variable | `OCR_LLM_USE_ANTHROPIC` | `false`: both endpoints are OpenAI-compatible. |
| Variable | `OCR_LLM_EXTRA_BODY` | `{"provider":{"only":["deepseek"],"allow_fallbacks":false}}`. Sent by the primary only. |
| Variable | `OCR_LLM_MODEL` | `deepseek/deepseek-v4.1-flash`. Set this last to enable the workflow. |
| Variable | `OCR_LLM_FALLBACK_MODEL` | `deepseek-ai/deepseek-v4.1-flash`. Omit to disable fallback. |

To go back to NIM alone, set `OCR_LLM_URL` and `OCR_LLM_AUTH_TOKEN` to NIM's,
`OCR_LLM_MODEL` to `deepseek-ai/deepseek-v4.1-flash`, delete `OCR_LLM_EXTRA_BODY`,
and set `OCR_LLM_FALLBACK_MODEL` to another NIM model or delete it.

No extra GitHub credential is needed: the workflow uses its short-lived `GITHUB_TOKEN`.
Never put an API key in this file, the workflow, a PR, or a command-line argument.
Use `gh secret set OCR_LLM_AUTH_TOKEN` with its hidden prompt or standard input.
The selected provider receives the PR diff and source context needed for review.
Review calls consume that provider's quota.

An unset `OCR_LLM_MODEL` skips the review job. A configured model with missing
secrets fails before installing or calling OpenCodeReview.

## Fallback behavior

OpenCodeReview 1.12.9 retries requests to its selected model but does not switch
models automatically. The workflow makes at most one additional review attempt
with the fallback model when the primary OCR CLI exits nonzero. Both attempts
review the same PR head with the same CLI version. The fallback uses
`OCR_LLM_FALLBACK_URL` and `OCR_LLM_FALLBACK_AUTH_TOKEN` when both are set, else the
primary's endpoint and key, and it sends no `OCR_LLM_EXTRA_BODY`. With only one of
the two set, the job fails before any review, so that no key goes to another
provider's URL.

Checkout, installation, configuration, and comment-publication failures do not
trigger fallback. Cancellation or the job timeout also stops the run. A primary
failure stays a failed check unless the fallback action succeeds; failures are
not silently ignored by `continue-on-error`.

A partially completed review can exit zero and publish its findings. That does
not trigger another model or duplicate the comments. Inspect the summary's
coverage and warnings. A failed primary CLI attempt does not publish its
findings; they remain in the log, and the fallback publishes its own result.

The fallback runs the same model elsewhere, so it covers an OpenRouter or
DeepSeek outage, an empty DeepSeek balance and an invalid OpenRouter key, but not
a fault of the model itself. It is tried only once.

## Triggers and limits

- Non-draft PRs whose head branch belongs to this repository run on open, push,
  reopen, and transition from draft to ready for review.
- Fork PRs do not consume quota automatically. A maintainer can review any open,
  non-draft PR from **Actions > OpenCodeReview > Run workflow**, selecting the
  default branch and supplying the PR number. For example:

  ```sh
  gh workflow run open-code-review.yml --ref main -f pr_number=123
  ```

- A newer eligible run for the same PR cancels its older queued or running run.
  Skipped fork/draft events and ordinary PR comments do not cancel eligible reviews.
- All PRs in this repository share one review slot. GitHub's native
  `queue: max` keeps up to 100 pending jobs, instead of replacing another PR's
  pending review. Jobs wait until the active review finishes; additional jobs are
  cancelled if that queue is full. Other applications or repositories using the
  same keys are outside this queue.
- At the start of a queued job, the workflow fetches the current PR metadata.
  Automatic reviews skip closed/draft PRs and superseded head/base snapshots.
  Manual reviews use the current head and still reject closed/draft PRs.
- Each attempt reviews one file group at a time, with a 600-second LLM request
  timeout and a 6,000,000-token budget. The CLI multiplies the fifteen-minute task
  setting by the three rounds of explicit `high` review effort, giving each file
  group a 45-minute deadline. Fallback can consume a second budget.
- The job's 120-minute timeout is a cap, not the sum of those deadlines: a review
  whose every file group runs to its 45-minute deadline is cut after about two
  and a half groups, and a cancelled job runs no fallback. The cap keeps one
  review from holding the slot that every PR shares.
- These limits are sized from local runs of OCR 1.12.9 on 2026-09-29. With the
  new 15-minute task setting but the old 500,000-token budget, NIM failed PR #218's
  one file: a planning call and 21 review calls, each resending the growing
  conversation, used 547,309 tokens (446,656 of them cached input), and the first
  round of three stopped at the budget after 11 minutes. On #225's seven files,
  NIM finished in 18 minutes with 983,770 tokens. DeepSeek's API made more calls
  (134, against NIM's 60), reading and searching more files: with a
  3,000,000-token budget it stopped in the third group after 8 minutes, two files
  unreviewed, and with 6,000,000 it finished in 9 minutes 16 seconds with 4,450,691
  tokens. PRs pushed close together wait about that long for each other in the
  shared slot.
- NIM's gateway answers 504 to a request still running at about 300 seconds, and
  OCR sends it again, so one slow call can cost a file group five minutes or more.
  DeepSeek's planning call on #218 got a 504 after 302 seconds, then answered in
  162 seconds: 7.7 of that run's 11 minutes. GLM's planning call on #218 (run
  36456002254) got two 504s, then answered in 227 seconds: about 14 minutes of
  the old 15-minute deadline, at which the file failed. The longer task setting
  leaves room for such retries.
  The CLI also checks an estimated file-group cost before dispatch. The initial
  100,000-token budget rejected all nine selected files in PR #147 before review:
  the first group was estimated at 249,216 tokens with GLM and 338,132 with Kimi.
  The budget is a soft stop, not a hard billing cap; an in-flight group or final
  round may exceed it, and unfinished files are reported.
- Findings are advisory. A successful job means the tool ran, not that the PR is
  defect-free or every file was reviewed. Inspect the summary for partial results.
- Existing inline findings are preserved. Each run may add findings on the same
  lines, so a different problem is not hidden by line-based deduplication.
  Review threads are not automatically resolved. Raw reports are not
  uploaded as artifacts; review output remains in the Actions log and PR comments.

`pull_request_target` and default-branch dispatch keep the working tree on trusted
repository code. The action fetches PR commits as git objects for review. This job
must not check out or run the PR's code. Its only write permission is
`pull-requests: write`; it cannot push commits or deploy the application.

## Validation and rollback

Validate the workflow against the current GitHub Actions schema and check its
expressions with `actionlint .github/workflows/open-code-review.yml` after edits.
Actionlint 1.7.12 has a known false positive for the supported `concurrency.queue`
property ([upstream issue](https://github.com/rhysd/actionlint/issues/657)); do not
remove the queue or ignore other diagnostics to accommodate that older schema.
GitHub's workflow parser and a current schema must accept the complete file.
After merging it to the default branch and configuring the provider, dispatch a
review of an open PR and verify the run's head SHA, review summary, and any inline
findings. The original CI remains the merge gate.

Existing runs retain their old concurrency groups when a workflow changes.
During rollout, let old OpenCodeReview runs finish or cancel them and requeue the
still-open PRs before relying on the shared slot. Leave other CI workflows alone.

To pause reviews, delete the `OCR_LLM_MODEL` repository variable. To remove the
integration, revert the commit that added this workflow and document. Provider
secrets can be removed once no workflow uses them.
