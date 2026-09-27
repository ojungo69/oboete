# OpenCodeReview in GitHub Actions

`.github/workflows/open-code-review.yml` runs the official
[Alibaba OpenCodeReview action](https://github.com/alibaba/open-code-review/blob/486022daaf14f7142275eddb9b3cacc3cc5dadfa/action.yml).
The action is pinned to a commit, and its CLI is pinned to version 1.12.9.
It posts Japanese inline findings and updates one summary comment on the PR.
It supplements the existing Rust CI and SonarCloud checks; it does not approve or merge PRs.

## Provider setup

In the repository's **Settings > Secrets and variables > Actions**, configure:

| Kind | Name | Value |
| --- | --- | --- |
| Secret | `OCR_LLM_URL` | Full LLM request endpoint, including `/chat/completions` or `/messages`. |
| Secret | `OCR_LLM_AUTH_TOKEN` | API key for the selected provider. |
| Variable | `OCR_LLM_USE_ANTHROPIC` | `true` for the Anthropic protocol; omitted or `false` for OpenAI-compatible APIs. |
| Variable | `OCR_LLM_MODEL` | Provider's model ID. Set this last to enable the workflow. |

No extra GitHub credential is needed: the workflow uses its short-lived `GITHUB_TOKEN`.
Never put an API key in this file, the workflow, a PR, or a command-line argument.
Use `gh secret set OCR_LLM_AUTH_TOKEN` with its hidden prompt or standard input.
The selected provider receives the PR diff and source context needed for review.
Review calls consume that provider's quota.

An unset `OCR_LLM_MODEL` skips the review job. A configured model with missing
secrets fails before installing or calling OpenCodeReview.

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
- Each job has a 30-minute timeout, two concurrent review tasks, and a 100,000-token
  budget. The upstream budget is a soft stop checked between LLM rounds, not a hard
  billing cap; a final round may exceed it, and unfinished files are reported.
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
