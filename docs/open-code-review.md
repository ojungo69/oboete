# OpenCodeReview in GitHub Actions

`.github/workflows/open-code-review.yml` runs the official
[Alibaba OpenCodeReview action](https://github.com/alibaba/open-code-review/blob/486022daaf14f7142275eddb9b3cacc3cc5dadfa/action.yml).
The action is pinned to a commit, and its CLI is pinned to version 1.12.9.
It posts Japanese inline findings and updates one summary comment on the PR.
It supplements the existing Rust CI and SonarCloud checks; it does not approve or merge PRs.

## Provider setup

This repository uses NVIDIA NIM: `z-ai/glm-5.3` is the primary model and
`moonshotai/kimi-k3` is the fallback. GLM is the starting choice for text-only
code review, not a claim that it outperforms Kimi on every repository. Both use
the same NIM account, OpenAI-compatible endpoint, and maximum `max` reasoning
effort. OCR's separate review effort is also at its maximum, `high` (three
review rounds); OCR does not accept `max` for that setting.

In the repository's **Settings > Secrets and variables > Actions**, configure:

| Kind | Name | Value |
| --- | --- | --- |
| Secret | `OCR_LLM_URL` | `https://integrate.api.nvidia.com/v1/chat/completions` |
| Secret | `OCR_LLM_AUTH_TOKEN` | NVIDIA NIM API key. |
| Variable | `OCR_LLM_USE_ANTHROPIC` | `false` for the NIM OpenAI-compatible endpoint. |
| Variable | `OCR_LLM_MODEL` | `z-ai/glm-5.3`. Set this last to enable the workflow. |
| Variable | `OCR_LLM_FALLBACK_MODEL` | `moonshotai/kimi-k3`. Omit to disable fallback. |

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
review the same PR head and use the same version, endpoint, and credentials.

Checkout, installation, configuration, and comment-publication failures do not
trigger fallback. Cancellation or the job timeout also stops the run. A primary
failure stays a failed check unless the fallback action succeeds; failures are
not silently ignored by `continue-on-error`.

A partially completed review can exit zero and publish its findings. That does
not trigger another model or duplicate the comments. Inspect the summary's
coverage and warnings. A failed primary CLI attempt does not publish its
findings; they remain in the log, and the fallback publishes its own result.

Changing models can help with model-specific failures. It cannot guarantee
recovery from shared NIM outages, account-wide quotas, or an invalid API key.
The alternate model is tried only once, even for these failures.

## Triggers and limits

- Non-draft PRs whose head branch belongs to this repository run on open, push,
  reopen, and transition from draft to ready for review.
- Fork PRs do not consume quota automatically. A maintainer can review any open,
  non-draft PR from **Actions > OpenCodeReview > Run workflow**, selecting the
  default branch and supplying the PR number. For example:

  ```sh
  gh workflow run open-code-review.yml --ref main -f pr_number=123
  ```

- A newer run for the same PR cancels an older run. Ordinary PR comments do not
  trigger or cancel reviews.
- Each job has a 45-minute timeout. Each attempt has two concurrent review tasks,
  a 300-second LLM request timeout, and a 500,000-token budget. The longer request
  and job limits leave room for maximum reasoning and the fallback attempt.
  The CLI multiplies the five-minute task setting by the three rounds of explicit
  `high` review effort, giving each file group a fifteen-minute deadline. The job
  deadline still applies across all groups and both models. Fallback can consume
  a second budget.
  The CLI also checks an estimated file-group cost before dispatch. The initial
  100,000-token budget rejected all nine selected files in PR #147 before review:
  the first group was estimated at 249,216 tokens with GLM and 338,132 with Kimi.
  The 500,000-token cap admits those groups while retaining a finite limit.
  It is a soft stop, not a hard billing cap; an in-flight group or final round may
  exceed it, and unfinished files are reported.
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

Run `actionlint .github/workflows/open-code-review.yml` after editing the workflow.
After merging it to the default branch and configuring the provider, dispatch a
review of an open PR and verify the run's head SHA, review summary, and any inline
findings. The original CI remains the merge gate.

To pause reviews, delete the `OCR_LLM_MODEL` repository variable. To remove the
integration, revert the commit that added this workflow and document. Provider
secrets can be removed once no workflow uses them.
