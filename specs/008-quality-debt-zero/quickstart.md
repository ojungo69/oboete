# Quickstart: verifying Quality Debt to Zero

**Plan**: [plan.md](./plan.md) | **Record format**: [data-model.md](./data-model.md)

## Prerequisites

- The two service-query blocks — "Service counts" and "Final analysis confirmation" — are read-only
  and anonymous: the repository is public, and SonarCloud's issue search and analysis search and
  Codacy's repository, commit and log endpoints all answer without credentials. Measured 2026-09-07
  against `f2f5b4be`: the authenticated and anonymous issue counts are the same number, 36, and the
  Codacy commit and log responses are byte-identical either way. So neither block reads, holds or
  transmits a token, and neither can leak one. This says nothing about the candidate-bundle block
  under "Per-batch verification", which sources `~/.oboete-credentials` under `set -a` so the dogfood
  harness can reach the agents' providers; those variables live in the `sudo -u oboete-dogfood` shell
  and are the account's own, not the service tokens this section is about.
- Credentials are needed only by the modes that authenticate to a service — `--apply-sonar`,
  `--apply-codacy` and `--confirm` — which read them themselves from `~/SONAR_TOKEN.md` and
  `~/CODACY_TOKEN.md` through `readToken` in `scripts/quality-debt-services.mjs`, and never print
  them. The SonarCloud token needs read scope + issue administration (verified 2026-09-07 with
  `additionalFields=transitions`).
- Codacy: anonymous read works for the public repository; per-issue ignores need an account API token (`CODACY_API_TOKEN`) for `npx @codacy/codacy-cloud-cli` (checkpoint C1 in the plan).
- Inventories: `sonar-main-issues.json` and `codacy-main-issues.json` exported on 2026-09-07 (session scratchpad; copied next to the disposition record when batch A opens).

## Per-batch verification (every pull request)

```bash
npm run typecheck && npm run lint && npm run build
node --test --enable-source-maps 'build/test/unit/**/*.test.mjs' 'build/test/migrations/**/*.test.mjs' 'scripts/e2e/**/*.test.mjs'
git diff --stat main...HEAD          # touches only the batch's scope
```

Expected: 843+ tests pass (count at 5e03d67f), no assertion edited (`git diff main...HEAD -- test/ | grep '^-.*assert'` is empty except for a real security fix's new test).

Hook-path batches (C1, C2, C3, D on `capture.ts` / `observe.ts` / `injection/*`), research R7 procedure: base SHA and candidate SHA, same quiet machine, same hour, each built in its own checkout, each exit code checked:

```bash
for sha in <base> <candidate>; do
  git worktree add /tmp/qd-$sha $sha || exit 1
  ( cd /tmp/qd-$sha && npm ci --no-audit --no-fund && npm run build \
    && node scripts/measure-cold-start.mjs > /tmp/qd-$sha.cold.md \
    && node dist/oboete.mjs fixture replay test/fixtures/events-1000.jsonl --json > /tmp/qd-$sha.replay.json ) \
    || { echo "FAILED on $sha"; exit 1; }
done
```

Expected: both loops exit 0; the candidate replay's own pass/fail is green; every median within 15 % of the base run; no series maximum more than 15 % above the base maximum and none over the 300 / 1,300 ms budgets. Paste the two Markdown tables and the two JSON results into the PR body.

Harness batch (C4, D on `scripts/e2e/*`), research R7 procedure: the agents' hook and MCP entries carry the absolute bundle path written by `oboete setup`, so the candidate needs its own install **and its own configured home**. `/home/jura` is `drwxr-x---`, so the harness is exported to a directory the dogfood account can read instead of being run out of the worktree:

```bash
npm run build && npm pack && sha256sum oboete-*.tgz && tar -xOzf oboete-*.tgz package/dist/oboete.mjs | sha256sum   # candidate hashes
sudo rm -rf /var/tmp/oboete-harness && sudo mkdir -p /var/tmp/oboete-harness \
  && git archive <candidate-sha> | sudo tar -x -C /var/tmp/oboete-harness && sudo chown -R oboete-dogfood: /var/tmp/oboete-harness
# Claude keeps its user configuration at ~/.claude.json, outside ~/.claude, and reads it from $CLAUDE_CONFIG_DIR;
# without that copy the run starts from an empty config and setup has nowhere to register the MCP server. The account's
# own bin directories go on PATH for the same reason: a non-login shell does not add them, and `oboete setup` skips the
# memory tools when `claude` is not on PATH.
# the harness refuses any HOME other than the account's own, and the agents' logins and oboete's consent record
# live in the real configuration directories, so HOME stays real and the five configuration variables point at copies
sudo -u oboete-dogfood -H bash -lc 'rm -rf ~/candidate ~/candidate-homes && mkdir -p ~/candidate ~/candidate-homes \
  && cp -a ~/.oboete ~/candidate-homes/oboete && cp -a ~/.claude ~/candidate-homes/claude && cp -a ~/.claude.json ~/candidate-homes/claude/.claude.json && cp -a ~/.codex ~/candidate-homes/codex && cp -a ~/.grok ~/candidate-homes/grok && cp -a ~/.pi/agent ~/candidate-homes/pi \
  && npm install --prefix ~/candidate <tarball> && sha256sum ~/candidate/node_modules/oboete/dist/oboete.mjs'
# A token refresh retires the old refresh token server-side, so a credential file the candidate run owns and then
# deletes leaves the account holding a token the server has already dropped -- on 2026-09-08 that cost the dogfood
# account its Claude login and needed an interactive re-login. Claude reads its credentials from the configured home
# for the whole run (`prepareClaudeAgent` copies only settings.json), so the link below is where a refresh lands --
# but the CLI writes through a temporary file and a rename, which replaces the link with a regular file, so the link
# is a place to look afterwards rather than a guarantee of write-through. The teardown step below copies the file back
# when that has happened. The other three agents cannot be covered at all: `prepareCodexAgent`, `prepareGrokAgent` and
# `preparePiAgent` copy `auth.json` again into each leg's own directory, so a refresh inside a leg is written to that
# copy and lost while the account's token is retired anyway (issue #175).
sudo -u oboete-dogfood -H bash -lc 'ln -sfn ~/.claude/.credentials.json ~/candidate-homes/claude/.credentials.json'
CAND='export OBOETE_HOME=$HOME/candidate-homes/oboete CLAUDE_CONFIG_DIR=$HOME/candidate-homes/claude CODEX_HOME=$HOME/candidate-homes/codex GROK_HOME=$HOME/candidate-homes/grok PI_CODING_AGENT_DIR=$HOME/candidate-homes/pi PATH=$HOME/candidate/node_modules/.bin:$HOME/.local/bin:$HOME/.npm-global/bin:$PATH; set -a; . $HOME/.oboete-credentials; set +a'
sudo -u oboete-dogfood -H bash -lc "$CAND; oboete setup --agents claude,codex,grok --provider workers-ai --yes --json"
# Pi is left out of --agents on purpose: setup declines to write the loader whenever PI_CODING_AGENT_DIR points
# away from $HOME/.pi/agent (src/setup/setup.ts:464, issue #174). Write the loader instead -- the paths must be
# absolute and expanded, which is why this is a command and not a block to retype. Without it Pi keeps the copied
# loader, which names the daily install, and its six pairs say nothing about the candidate.
sudo -u oboete-dogfood -H bash -lc 'B=$HOME/candidate/node_modules/oboete/dist; D=$HOME/candidate-homes/pi/extensions; mkdir -p $D; umask 177; { echo "// oboete:managed written by \`oboete setup\`; \`oboete setup --remove\` deletes it."; echo "import { piExtension } from \"file://$B/pi-extension.mjs\";"; echo "export default (pi) => piExtension(pi, { node: \"$(command -v node)\", bundle: \"$B/oboete.mjs\" });"; } > $D/oboete.js'
# The check names the configuration files `oboete setup` writes, plus the loader above. `.claude.json` carries the
# MCP registration, which `settings.json` does not, so leaving it out hides a whole class of mis-wiring.
# The count is asserted exactly: a grep that fails, or a file that stops being written, must not read as a pass.
# The check names the configuration files `oboete setup` writes, plus the loader above. A recursive scan of the
# copied homes cannot work: session transcripts, rotated backups and agent history quote whichever bundle path they
# were written under, and no exclusion list stays right as the agents add directories. The counts go through a
# variable rather than `tee /dev/stderr`, which is not writable when the command runs under `sudo -u`.
sudo -u oboete-dogfood -H bash -lc "$CAND; set -o pipefail; refs=\$(grep -hoE \"[^\\\"' ]*node_modules/oboete/dist/oboete[.]mjs\" \
  \$CLAUDE_CONFIG_DIR/settings.json \$CLAUDE_CONFIG_DIR/.claude.json \$CODEX_HOME/config.toml \$CODEX_HOME/hooks.json \$GROK_HOME/config.toml \$GROK_HOME/hooks/oboete.json \$PI_CODING_AGENT_DIR/extensions/oboete.js | sort | uniq -c) || { echo 'bundle reference grep failed'; exit 1; }; \
  printf '%s\n' \"\$refs\"; printf '%s\n' \"\$refs\" | awk -v want=\"\$HOME/candidate/node_modules/oboete/dist/oboete.mjs\" -v expect=28 'BEGIN { n = 0 } \$2 != want { print \"non-candidate bundle reference: \" \$2; bad = 1 } { n += \$1 } END { if (bad || n != expect) exit 1 }'"
sudo -u oboete-dogfood -H bash -lc "$CAND; node /var/tmp/oboete-harness/scripts/e2e/isolated-user.mjs --daily --pairs all"
# Required whenever lifecycle code or the TUI changed: `--pairs` never enters runCompactLifecycle,
# runForkLifecycle or runClearLifecycle, so a batch that touches them is unverified without this run.
sudo -u oboete-dogfood -H bash -lc "$CAND; node /var/tmp/oboete-harness/scripts/e2e/isolated-user.mjs --lifecycle --agents claude,codex"
sudo -u oboete-dogfood -H bash -lc "$CAND; oboete doctor --probe-provider"   # the harness prints no doctor table; this is where it comes from
# If the CLI replaced the symlink, the run holds the only token the provider still accepts; put it back before
# deleting the copies, or the account is locked out exactly as it was on 2026-09-08.
sudo -u oboete-dogfood -H bash -lc '[ -f ~/candidate-homes/claude/.credentials.json ] && [ ! -L ~/candidate-homes/claude/.credentials.json ] && cp -a ~/candidate-homes/claude/.credentials.json ~/.claude/.credentials.json; rm -rf ~/candidate ~/candidate-homes' && sudo rm -rf /var/tmp/oboete-harness
```

Expected: the installed bundle's `sha256sum` equals the tarball's `dist/oboete.mjs` hash; `oboete setup` exits 0 for the three agents it can wire (the copied consent record satisfies `--yes`); the `uniq -c` output lists exactly one distinct bundle path, under `candidate/node_modules/oboete/dist/`, with a count of exactly 28 across the seven files -- 8 in Claude's `settings.json` and 1 in its `.claude.json`, 1 + 7 in Codex's `config.toml` and `hooks.json`, 1 + 9 in Grok's, 1 in the Pi loader (a second path, or one under the daily install, fails the check); `12 of 12 pairs pass`; from the lifecycle run, every check `pass` for both agents; `oboete doctor --probe-provider` exits 0 with every `agent:` row healthy, which is what says the candidate is the bundle the agents actually loaded; `catalog unverified` and the `native-memory:claude` warning are informational and do not affect the exit. That command inspects `candidate-homes/oboete`, copied from the daily `~/.oboete` at the start of the run, not the per-pair databases the pairs write — so if `worker` reads degraded, establish whether that lease came from this run before calling it a leftover. A failing row is not evidence about the bundle until the agent's own `result` field has been read: an expired login fails a pair exactly the way a broken build does. Candidate SHA, both hashes, run id, and both lines go into the PR body. The daily cron's install, its real `~/.oboete`, and its agent configurations are untouched; the copies are removed afterwards.

## Service counts (after each merge's analysis)

```bash
curl -s 'https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=1' | python3 -c 'import sys,json; print("sonar open", json.load(sys.stdin)["total"])'
curl -s -X POST 'https://app.codacy.com/api/v3/analysis/organizations/gh/ojungo69/repositories/oboete/issues/search?limit=1' -H 'content-type: application/json' -d '{}' | python3 -c 'import sys,json; d=json.load(sys.stdin); print("codacy current", d["pagination"].get("total", len(d["data"])))'
```

Expected trajectory: Sonar 310 → ≈ 295 (A) → ≈ 95 (B) → ≈ 30 (C) → 0 (F). Codacy 397 → ≈ 112 (A) → ≈ 40 (C) → ≈ 30 (D) → 0 (F). The Quality Gate conditions on `main` stay `OK` and `coverage` stays ≥ 93.3 %.

## Disposition record check (every batch, and batch F)

```bash
node scripts/quality-debt-record.mjs --check --planned   # between batches: every id has a planned end state
node scripts/quality-debt-record.mjs --check             # batch F: every planned state is confirmed by the service
```

Expected: `--check --planned` exits 0 once every batch has merged; `--check` exits 0 only at the end of F with `720 ids: 0 missing, 0 duplicate, 0 open, 0 unconfirmed, 0 resolved-without-reason`. A random sample of 20 rows traces to a PR, a service comment, or a configuration line.

## Final analysis confirmation (batch F, before writing 0 / 0)

```bash
SHA=$(git rev-parse origin/main)
curl -s "https://sonarcloud.io/api/project_analyses/search?project=ojungo69_free-mem&branch=main&ps=1" | python3 -c "import sys,json; a=json.load(sys.stdin)['analyses'][0]; print(a['key'], a['revision'], a['date']); assert a['revision']=='$SHA', 'latest Sonar analysis is not the final SHA'"
B="https://app.codacy.com/api/v3/analysis/organizations/gh/ojungo69/repositories/oboete/commits/$SHA"
curl -s "https://app.codacy.com/api/v3/analysis/organizations/gh/ojungo69/repositories/oboete" | python3 -c "import sys,json; c=json.load(sys.stdin)['data']['lastAnalysedCommit']; print(c['sha'], c.get('endedAnalysis')); assert c['sha']=='$SHA' and c.get('endedAnalysis'), 'Codacy last analysed commit is not the final SHA'"
curl -s "$B" | python3 -c "import sys,json; c=json.load(sys.stdin)['commit']; print(c['sha'], c.get('startedAnalysis'), c.get('endedAnalysis')); assert c['sha']=='$SHA' and c.get('endedAnalysis'), 'Codacy has not finished analysing the final SHA'"
curl -s "$B/logs" | python3 -c "import sys,json; s={x['title']: x['status'] for x in json.load(sys.stdin)['data']['steps']}; print(s); need=['Opengrep','Lizard','markdownlint','Stylelint','ShellCheck','TSQLLint','SQLint']; bad=[t for t in need if s.get(t)!='success']; assert not bad, 'steps not successful: %s' % bad; other=[t for t,v in s.items() if v!='success' and t!='ESLint']; assert not other, 'other steps not successful: %s' % other; assert 'ESLint' not in s, 'ESLint step still runs (research R10: the tool is disabled by decision C3)'"
```

Expected: the Sonar analysis `revision` equals the final SHA; the repository's `data.lastAnalysedCommit` is the final SHA with an `endedAnalysis` timestamp (`--confirm` re-checks exactly this and refuses with `Codacy: the last analysed commit is not <sha>` otherwise, because the issue search has no commit selector; it also requires the Sonar analysis `revision` to be the same SHA and refuses with `Sonar: analysis <key> is not of commit <sha>` otherwise; neither message repeats a value from a response, run the curl above to see it); the Codacy commit response (`{commit, quality, coverage, meta}`, shape read from the live API on 2026-09-07) has `commit.sha` equal to the SHA and an `endedAnalysis` timestamp; the analysis log lists every required step as `success` and no ESLint step at all (research R10: the tool crashed on every commit and is disabled by decision C3; a log that still shows it means the disable did not take). Every assert exits non-zero otherwise. Only then run the two count commands and record 0 / 0 with these ids. No repository edit follows this SHA: the polish tasks are part of the last pull request.

## Gate definitions unchanged (SC-006)

```bash
git diff 5e03d67f..HEAD -- .github/workflows/ci.yml .github/workflows/*.yml sonar-project.properties .coderabbit.yaml
```

Expected: in `sonar-project.properties` the `sonar.plsql.file.suffixes` line, `**/*.test-support.mjs` added to `sonar.test.inclusions` (the record tests' shared fixtures are test code), and no `scripts/quality-debt-*.mjs` module added to `sonar.coverage.exclusions`, plus the one comment line above the `plsql` line; in `ci.yml` only the Engine coverage step's second `node --test` invocation for `scripts/quality-debt-record*.test.mjs` (same `NODE_V8_COVERAGE` directory) and its two comment lines. No threshold, quality-gate condition, required check, or workflow trigger changes; every other line of these files is byte-identical. Anything beyond this list fails SC-006.
