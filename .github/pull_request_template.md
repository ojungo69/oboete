## Summary

Describe what changed and why. Keep the scope narrow enough to review and revert safely.

## Validation

List the commands, fixtures, or manual checks used to validate this change.

- [ ] `node --test scripts/dco-check.test.mjs` (and `npm ci && npm run build && npm test` once `package.json` exists)
- [ ] Other validation is documented below, or a reason is given for skipped checks.

## Constitution compliance

- [ ] This change complies with Principles I-VI of `CONSTITUTION.md`, or the exception is
      named and approved below.
- [ ] No real credentials, private memory content, database files, or private local paths are included.
- [ ] Fail-open capture, fail-closed classification, secret redaction, and the egress table remain
      intact, or their changes are explicitly documented.
- [ ] Breaking schema, CLI, configuration, migration, or hook-contract changes are identified.
- [ ] New or changed third-party material has clear provenance and its notices are updated when required.
- [ ] Documentation and tests were updated where behavior changed.

## Provenance and license

- [ ] Every commit is signed off (`git commit -s`), certifying the [DCO](https://developercertificate.org/).
- [ ] This contribution is offered under the repository's license (Apache-2.0, inbound = outbound).
- [ ] No third-party code was copied in without recording its upstream URL, commit, and license, and
      without updating `THIRD_PARTY_NOTICES.md` in this same pull request.
- [ ] AI assistance, if any, is declared below (which tool, and confirmation that the output was
      reviewed and does not reproduce third-party code).

## Additional context

Include relevant issue links, screenshots, benchmark results, migration notes, or rollback instructions.
