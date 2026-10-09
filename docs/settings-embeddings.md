# Embeddings on the settings page (W4)

Settings W4 lets the Japanese and English settings page choose the embedder (`none`, `local` or
`workers-ai`), set Workers AI's account, caps and write-only token, download the local model
after showing what it fetches, and show the state of the local model's files. It follows the rules
`oboete setup --embeddings` already keeps (spec 7.1, 7.2): a choice says what leaves the machine
before anything changes, `local` downloads and checks its files before it is chosen, and only
`[embedding] provider` changes for the choice itself. Opening the page sends nothing anywhere and
downloads nothing. The code is `src/settings/embedding.rs`; the page's part is the "Search by
meaning" group of `assets/viewer/app.js`.

## What the page shows

`GET /api/settings` gains `embedding`:

- `provider`: the saved choice.
- `workers_ai`: `account_id`, `key_file`, `key` (`ok` when the token's file is there, `missing`
  otherwise; never the token), `daily_requests`, `monthly_usd`, and this month's embedding spend
  and the requests of the last 24 hours as doctor counts them from providers.db (`null` when the
  ledger cannot be read).
- `local`: `unavailable` (`no_feature` for a build without `local-embed`, `no_runtime` where
  Microsoft releases no ONNX Runtime 1.28.0), else `files` from the files' sizes and the
  `verified` marker, hashing nothing (`ready`, `not_downloaded`, `incomplete` or `changed`, as
  doctor's line says them: `embed::files_in`); `dir` and `held`, the bytes in the folder.

The documents still waiting for a vector are not counted here: counting them reads the whole
store, and doctor's "Embedding work" group already shows them.

`GET /api/embedding` answers `{active, last, held}`: the choice running, if one is, the last one
this viewer ran, and the bytes the model folder holds, which grow while a download runs. A run is
`{choice, phase, code, held, get}`: `phase` is `running`, `done`, `failed` or `unknown`, and
`held` and `get` are the bytes held and still to download when it started. The page reads it when
the settings open and with its three-second poll while a run is active, and words the progress as
the bytes downloaded of those to get.

## Choosing the embedder

The choice is its own action, not part of the settings save, as it may download 2.3 GB and as its
consent is about where texts go. What a choice does is computed once, by the function `oboete
setup --embeddings` uses (`setup::consent`): the CLI prints its sentence, and the viewer answers
its fields, which the page words in Japanese or English. So spec 7.2's "what leaves the machine"
has one implementation.

`POST /api/embedding/preview {"choice"}` reads only: no request goes out, and no file or folder is
made. It answers `{preview_key, consent}`. The key is a random nonce the viewer keeps with a
fingerprint of config.toml's version and the consent, as the recovery preview does
(`settings/recovery.rs`). The consent is one of:

- `{"choice": "none", dir, held}`: nothing leaves the machine; the vectors stay in the store; the
  local model's files stay in their folder, which the page does not delete.
- `{"choice": "workers-ai", account, key_file, daily_requests, monthly_usd}`: each memory's text
  after redaction and each search query go to Cloudflare Workers AI under that account, with the
  token in that file, within those caps.
- `{"choice": "local", dir, get, check}`: `get` lists the bytes still to download per host and
  source (`model`: BAAI's bge-m3 from huggingface.co; `runtime`: Microsoft's ONNX Runtime from
  github.com; both MIT). With nothing to get, `check` says whether the files there are checked
  against their pins first. Afterwards no text or query leaves the machine to be embedded.

A choice that cannot be taken now is refused with 422 and the field `embedding.choice`:
`no_account` (Workers AI without `[embedding] account_id`), `no_key` (its token file does not
read) or `local_unavailable`. A config.toml one of its readers would refuse is `file_invalid`.
While a choice runs, a preview is `embedding_busy` (409).

`POST /api/embedding {"choice", "preview_key", "confirmed": true}` takes the preview. Under the
viewer's settings lock and config.lock it computes the consent again and compares the fingerprint:
a key used before, or a config.toml or files changed since the preview, is `stale` (409), and a
missing agreement is `embedding_confirmation` (422). The key is used once, whatever happens next.

- `none` and `workers-ai` are written at once, under the holds the check was made under, as
  `settings::set_embedding_provider` writes (every reader's parse, then the version check).
- `local` with files to download or check runs with only the model folder's lock held
  (`setup::ready`: `model_fetch::fetch`, `fetch_runtime`, `verify`), so every other settings save
  goes on meanwhile, and a `setup --embeddings` at the same time finds the folder busy. Then the
  two holds are taken again to write the provider. The request's answer waits for the run; the
  page reads its progress from `GET /api/embedding` on other connections, and a start whose answer
  does not come back (a browser can stop waiting) is followed by the poll until the run ends.

The answer is the status. A run that stops says why in `last.code`: `embedding_busy` (another
oboete holds the model folder), `embedding_no_space` (less free space than the download needs),
`embedding_failed` (any other download or check failure; `oboete setup --embeddings local` shows
its details), `embedding_write_failed` (the files are ready but config.toml was not written) or
`embedding_unknown` (the run stopped in a panic). A stopped download changes no setting: the files
stay, their state says `incomplete`, and a new choice resumes it. The viewer runs one choice at a
time and keeps the last one, as maintenance keeps its receipt (`settings/maintenance.rs`).

## Workers AI's account, caps and token

`account_id`, `daily_requests` and `monthly_usd` join the page's settings save as `embedding`
(optional, so an older page's save leaves them), checked as the page checks its other values: an
account id is Cloudflare's 32 lowercase hexadecimal characters (the page lowercases what is typed),
`daily_requests` 1 to 100,000 (the provider budget's range), and `monthly_usd` a number above 0,
as config.toml requires. Each is written only when it changes, comments kept; an empty account
field keeps the saved account, as a `workers-ai` choice needs one. The fields are shown open while
`workers-ai` is the choice.

The token is write-only: `POST /api/embedding/key {"key", "version"}`, under the same guards and
the same 1 KiB body cap as `/api/providers/key`, registers it as a provider's key is registered
(`keyfile::managed`: a new owner-only file outside the store) and points `[embedding] key_file` at
it; the file it was read from before stays as it was. The answer is the settings with the key's
state, never the key. Registration is Linux only until #281, as for providers: on macOS and
Windows the page shows the key state without a field (the iMac, the one Workers AI machine, keeps
its token in the default `key_file`, which shows `ok`). This endpoint is security scope: its PR
runs semgrep and a security review.

## The index

Workers AI and the local runner share one generation, `bge-m3` (milestone 4 D8), so switching
between them keeps every vector, and switching to `none` keeps them for a later choice. The page
says so. No choice offered here builds a new generation; one would come only with another model
(#396), which W4 does not offer.

## Not in W4

Deleting the local model's files from the page: the `none` review and the CLI's `none` line name
the folder and its size. Per-prompt injection of past work by meaning (G23) is its own change.

## Tests

- `settings/embedding.rs`: `the_page_shows_the_choice_the_token_state_and_the_local_files` (each
  local state, the token's state without the token); `a_choice_is_previewed_and_then_taken_once`
  (refusals before any key, a preview makes nothing, a key used once even for a choice that changes
  nothing, the agreement, a config edited after the preview); `local_is_written_only_once_its_files_are_checked`
  (with `local-embed`: files checked then written; a failed download and a held folder write
  nothing; while a download waits on its server, the page sees it running, a settings save
  finishes before it, and another choice is busy); `workers_ai_values_are_checked_and_written_alone`;
  `a_workers_ai_token_goes_to_its_own_file_and_into_no_answer` (Linux).
- `setup.rs` `setup_embeddings`: `setup --embeddings` says and does the same as before.
- `view.rs`: the three POST routes pass every guard first, the token's at 1 KiB.
- The viewer harness (`src/testdata/viewer-readiness/test.mjs`): both languages for every string,
  the review before the agreement, the run while its answer waits and its stopped reason, the
  values checked before a save, and the token in no page state.
