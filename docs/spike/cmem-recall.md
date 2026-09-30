# claude-mem's recall on M3's dev labels (2026-09-30)

What the owner's own claude-mem kept of the owner's 45 labeled dev decisions (value yes; 44 of them are M3's recall set, d424 is not), to place M3's recall line against it. docs/milestone-3.md reads the result. The labels, claude-mem's records and the judges' reasons stay outside the repository, with the labels (`~/.oboete/eval/labels`, `oboete-work/cmem-baseline`): they hold the owner's words and projects.

## The rubric (fixed 2026-09-30 13:35Z, before any judge ran)

**Candidates** (`docs/eval/cmem_candidates.py`):

- The claude-mem database is the research copy `~/.oboete/eval/claude-mem-2026-09-24.db` (152,075 observations, 2025-12-14 to 2026-09-24). All 31 labeled sessions are in it, Codex's too.
- A label's prompt N: the session's user prompt whose text holds the label's quote, whitespace aside (28 labels); else the session's last prompt before the label's time (16: a pick in AskUserQuestion, or a line inside a longer turn). One label (d158) has neither.
- Its candidates: every observation and session summary claude-mem made in that session for prompt N and prompt N + 1. N + 1 is generous to claude-mem: a decision's work often lands in the next turn, where oboete's recall counts only a claim quoting the label's own record.
- The judges see each record's fields cut to 300 characters (a summary's to 200 to 400). Four labels have no candidate: d158, d11, d449, d380.

**Kept**: at least one candidate states the owner's decision, the choice the label's statement describes or an outcome that names that choice (for 「CCSを完全削除して」, "Removed CCS completely" counts, "Investigated CCS" does not). It need not quote the owner. The test: a later session that reads only that record would know the owner chose this. Not kept: the candidates name only the topic, the question, the options, a different choice, or work that does not show the choice.

**Judging**: two Sonnet judges per label, each reading every part of its candidates on its own (a label over 100,000 characters is split into parts, kept when any part is); where they disagree, Opus reads the records they cited and decides. The judges see the label and the candidates, not oboete's result.

## Result

| | Labels |
|---|---|
| Kept, of 45 | 25 |
| Kept, of M3's 44 | 24 |
| Kept, of the 41 lasting (the three instructions for the moment left out) | 22 |
| The two judges agreed | 37 of 41 (Opus decided d291, d363, d365, d332) |

Kept: d294, d415, d352, d147, d34, d107, d112, d427, d211, d293, d291, d424, d130, d200, d173, d373, d146, d363, d46, d54, d360, d333, d351, d315, d332.

Not kept: d446, d490, d331, d435, d43, d172, d215, d462, d335, d438, d365, d439, d181, d217, d50, d300, and the four with no candidate.

Claude read the cited records of all 25 keeps. 3 hold only on the rubric's outcome reading (d147: the branch merged to main; d291: a runner-led update process; d427: the session waiting for the review slot), so a stricter reading gives 21 of 44.

The records were made mostly by free OpenRouter models and Haiku (`generated_by_model` over the labeled sessions: stepfun step-3.7-flash, nvidia nemotron free, Haiku 4.5, google gemma free, and others).
