# Stopped curation: the cause, the remedy, the relay (parity row 11)

When curation stops, a session's start says so (G22). This note covers what parity row 11 added on
2026-10-25: the cause only the owner can end, its remedy, the line that asks an agent to tell the
person, and a saved key ending its rest.

## What claude-mem does

`src/shared/observer-health.ts` at v15.1.0:

- **When**: the observer is unhealthy after three failures in a row, or at once after a refused
  credential or a setup failure (a missing CLI or login, a model the provider does not serve), "the
  one remedy (a new key)" not hidden for an hour behind the threshold (`isObserverUnhealthy`).
- **What**: each session's start context carries a warning: the provider, how long it has failed,
  the provider's error words (credentials scrubbed), its remedy and link, what is lost meanwhile, and
  a remedy by class. A spent allowance, a retired model and refused credentials each say that a
  restart does not help. Any other failure gives a restart link and `npx claude-mem doctor`
  (`renderObserverHealthWarning`).
- **Relay**: the warning ends by asking the assistant to tell the user about the outage at the very
  start of its first reply.

## What oboete does

- **What stopped** is G22's line, as before: the window every provider failed while it waits for
  the owner, on a budget, or on resets once it has waited 30 minutes (`curate::STOPPED_MS`; three
  counted attempts skip a window), and the spans every provider skipped.
- **Causes only the owner ends**: each enabled entry of the chain that rests, whose last call's
  detail in providers.db (`provider_calls.detail`: a status and a vetted code, never the provider's
  words, issue #91) names one of these (`provider::Cause::of`):
  - a refused key: 401 or 403 with `invalid_api_key`, `authentication_error`, `permission_error` or
    `PERMISSION_DENIED`, the same test as the chain's `key_rejected`;
  - spent credits: 402, Anthropic's credits spent, `insufficient_quota` or `billing_error`;
  - a missing model: `model_not_found`, or Anthropic's `not_found_error` on a 404.
  An entry skipped while it rests writes no row, so its last row is the call that set the rest.
- **Said at once**: a window that waits on time is said at once while such an entry rests, as
  claude-mem says a refused key at once. An outage or a rate limit still waits its 30 minutes.
- **The remedy**, in the agent's line and in the terminal note (in the summary language, Japanese
  or English), with the entry's name, which is the owner's config, never the provider's words:
  - a refused key: put a new key on the settings page; the next pass uses it;
  - spent credits: add credits with the provider, or turn the entry off on the settings page;
  - a missing model: choose another model on the settings page.
  A wait on time whose cause is the owner's no longer says that the providers do not answer.
- **A saved key ends its rest**: a key saved on the settings page clears the rests of the entries
  that read it (the simple page writes one key file, which every entry naming it reads; the
  providers page binds a new file to one entry), and the windows and session summaries that wait
  on time or on the owner are tried at the worker's next pass (`providers_db::key_saved`). A wait
  on a budget stays: a key lifts no cap. Before, a refused key's rest doubled up to a day, and a
  new key waited it out. The key is saved even when providers.db cannot be written.
- **The relay**: an agent no terminal note reaches is asked to tell the user about the stop at the
  start of its first reply: Codex, Grok, and `oboete inject` (OpenCode), and Claude Code when its
  terminal note is turned off. Claude Code with the note on shows the person the note itself, and
  its agent is not asked.
- **Doctor** gives the same cause beside an entry's rest.

## Differences from claude-mem

- **No provider's words, link or request id.** A provider's error body is read once and never
  kept or shown (issue #91), so oboete says the class and its own remedy. The owner sees each
  entry's state on the settings page and with `oboete doctor`.
- **No restart link.** Each window is sent afresh at each attempt, so no observer process is left
  to restart; what waits is tried again by itself, or at once once the owner acts.
- **Several entries.** claude-mem's warning is about its one observer. oboete names every entry of
  the chain that rests on such a cause, and only while a window every entry failed waits: an entry
  that fails while another answers is in doctor and on the settings page, not in a session's start.

## Tests

- `provider::tests::a_rests_cause_is_read_from_the_vetted_detail`
- `curate::tests::a_refused_key_is_said_at_once_with_its_entry_and_remedy`
- `curate::tests::a_sessions_start_says_why_curation_stopped` (the existing classes, unchanged)
- `providers_db::tests::a_saved_key_ends_its_entries_rests_and_wakes_waits_on_time_or_the_owner`
- `settings::tests::a_key_saved_from_the_page_reaches_its_file_and_no_answer` (the entries that
  read the file, and no other)
- `settings::providers::tests::a_key_saved_on_the_providers_page_ends_its_entrys_rest`
- `hook::tests::session_start_says_why_curation_stopped` (no relay beside Claude Code's note, a
  relay for Codex and in `oboete inject`)
