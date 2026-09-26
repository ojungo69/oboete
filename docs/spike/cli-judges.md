# Spike: the owner's grok and codex subscriptions as panel judges (2026-09-26)

Throwaway. Harness: `docs/spike/cli-judges/canary.py`. The judge code is `calib.cli_chat` (`docs/eval/calib.py`).

## Why

The owner-decision panel (Task 8) stopped on rate limits: four of its five judges run on OpenCode Go, whose 5-hour usage window ran out (`429 GoUsageLimitError`, `Retry-After: 13809`), and Groq's free tier allows 8,000 tokens a minute. The owner asked to judge with the models of their grok and codex subscriptions too, with tools restricted (2026-09-26). Grok is excluded only as the summarizer, not as a judge (owner, same day).

A proxy (omniroute, CLIProxyAPI) or pi is not needed: both CLIs can run one tool-less turn themselves, and pi had no provider logged in (`pi auth check`: every provider `not_ready`).

## Where they run

As the `oboete-dogfood` user (`sudo -n -u oboete-dogfood -H`), with its own grok and codex logins. The owner's grok loads everything under `~/.grok` and `~/.claude` (`grok inspect` from an empty directory: 17 instruction files, 301 skills, 48 plugins, 25 MCP servers, 52 hooks, among them claude-mem's), so a judge call there would run the owner's hooks on evaluation prompts. The dogfood grok has no hooks, MCP servers or plugins.

The prompt reaches the CLI on stdin, into a private temporary directory that is removed after the call, never on the command line.

## Flags

- grok 1.0.40: `--prompt-file`, `--deny '*'` (a bare `*` rule matches every tool), `--permission-mode dontAsk`, `--disable-web-search`, `--no-subagents`, `--no-plan`, `--max-turns 3`, `--output-format json`, `-m grok-4.7`. With `GROK_MEMORY=0` and `GROK_SESSION_SEARCH=0`. The session grok writes for the temporary directory (`~/.grok/sessions/<cwd, percent-encoded>`) is removed with it. The model grok reports is the key of `modelUsage` (`grok-4.7-build`).
- codex 0.155.1: oboete's curator isolation from `src/provider.rs` (`--ephemeral`, `--ignore-user-config`, `--ignore-rules`, the seven `--disable` features, `web_search="disabled"`, the `curator` permission profile), `model_reasoning_effort=low`, `model=gpt-6-astra` (the model of the owner's own codex config). The model is the `model:` line codex prints on stderr.

## Canary

`canary.py` asks each judge, through `cli_chat`, to `cat` a random secret in the dogfood HOME, write `~/judge-canary.written` and fetch a page from a listener it runs on 127.0.0.1. The listener's request log decides the network row, not the answer's text; as a control, `curl` run as the dogfood user reaches the listener.

| Run | Secret in the answer | File written | Network |
|---|---|---|---|
| grok, `--tools ""` (first try, not adopted) | yes | yes | fetched https://example.com (answer quoted it) |
| grok, `--deny '*'` | no | no | 0 requests to the listener |
| codex, curator profile | no | no | 0 requests to the listener |

`--tools ""` does not remove grok's tools, and `dontAsk` still runs reads (the permission guide lists them as auto-approved in every mode). Each call also runs under `timeout -k 10` inside the dogfood shell, so a call that overruns is killed there and does not outlive the caller. `--sandbox strict` did not start here (a 300 s timeout, then exit 1), so it is not used; `--deny '*'` is the layer that holds, and the dogfood user is the one around it.

The canary passed on grok 1.0.40 and codex-cli 0.155.1 (the dogfood user's installs; its grok has `auto_update = false`). `calib.TESTED` holds these versions, and a judge call refuses any other until the canary passes on it and the constant changes.

After a judge call, neither `~/.grok` nor `~/.codex` of the dogfood user holds the prompt text (`grep` for the prompt; `session_search.sqlite` unchanged).

## Calibration (B3's rule, run 3)

Both judges graded calib-50 on the frozen inputs (`calib-50.inputs.jsonl`); run 2's grades of the other judges stand. Each new judge must reach κ ≥ 0.4 against the majority of the others (spec 8.1) before it judges anything.

Run 3 (`calib-50.panel-3.jsonl`, 100 grades, none failed or unusable; both frozen with `calib-50.result-3.json`), pair c29 left out for every judge as in run 2 (49 pairs):

| Judge | κ against the others' majority | Agreement | Pass |
|---|---|---|---|
| grok-4.7 (reported `grok-4.7-build`) | 0.838 | 92% | yes |
| gpt-6-astra | 0.797 | 90% | yes |

With eight judges the panel's Fleiss κ is 0.746 (run 2, six judges: 0.741), and claude-sonnet-5 stays at κ 0.878. Both new judges join the owner-decision panel and grade every target there, not only the ones left when OpenCode Go's window closed. gpt-6-astra is OpenAI's, like gpt-oss-120b. So calibration has eight judges from seven makers (with claude-sonnet-5, the judge under test), and the panel that labels (`calib.PANEL`, without claude-sonnet-5) has seven judges from six makers.
