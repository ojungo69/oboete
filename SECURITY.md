# Security policy

oboete records what coding agents see and do on your machine, masks secrets before it stores or
shows anything, and sends records to the AI providers you configure. A way past the masking, a way
for records to reach somewhere they should not (a provider you excluded, another repository's
session, a page of another site), or a way to make oboete run or write something it should not, is
a vulnerability.

## Reporting a vulnerability

Report it in private, through GitHub: open this repository's **Security** tab and choose
**Report a vulnerability**
([the form](https://github.com/ojungo69/oboete/security/advisories/new)).

- Please do not open a public issue or pull request for it.
- Leave real secrets out of the report: a made-up value of the same shape shows the problem as
  well.
- What helps: the version (`oboete --version`), the operating system, the agent (Claude Code,
  Codex, ...), what you did, and what happened.

## What happens next

- The report is answered in its advisory, which stays private until a fix is out.
- A fix lands on `main` and is published as a GitHub Security Advisory; the notes of the build
  that carries it say so.
- oboete is not finished: it has trial builds, no release yet, and one maintainer. No response
  time is promised, and there is no bounty.

## Which versions

`main`, and the newest trial build. An older trial build gets no fix of its own: install the
newest.
