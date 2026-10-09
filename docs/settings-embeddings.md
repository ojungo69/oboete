# Embeddings on the settings page (W4)

Settings W4 lets the Japanese and English settings page choose the embedder (`none`, `local` or
`workers-ai`), set Workers AI's account, caps and write-only token, download the local model
after showing what it fetches, and show the index's state. It follows the rules `oboete setup
--embeddings` already keeps (spec 7.1, 7.2): a choice says what leaves the machine before
anything changes, `local` downloads and checks its files before it is chosen, and only
`[embedding] provider` changes for the choice itself. Opening the page sends nothing anywhere
and downloads nothing.

## What the page shows

`GET /api/settings` gains `embedding`:

- `provider`: the saved choice.
- `workers_ai`: `account_id`, `key` (`ok`, `missing` or `none`, as a provider entry's key; never
  the token), `key_file`, `daily_requests`, `monthly_usd`, and this month's embedding spend and
  today's requests from providers.db (the doctor's numbers).
- `local`: whether this build and machine can run it (`unavailable`: `no_feature` or
  `no_runtime`), the files' state from their sizes and the `verified` marker, hashing nothing
  (`ready`, `not_downloaded`, `incomplete` or `changed`), the folder, the bytes held and the bytes
  a download would fetch per host, whether this viewer is downloading, and the last download the
  page started (`ok`, or a failure code with its message).
- `index`: the active generation's embedder, the documents waiting for a vector per kind, and the
  current runner's rest (`resting_until`), as doctor reads them.

## Choosing the embedder

The choice is its own action, not part of the settings save, as it may download 2.3 GB and as
its consent is about where texts go. What a choice does is computed once, by the function
`oboete setup --embeddings` uses (`setup::consent`): the CLI prints its sentence, and the viewer
answers its fields, which the page words in Japanese or English. So spec 7.2's "what leaves the
machine" has one implementation.

`POST /api/embedding/preview {"choice"}` reads only: no request goes out, no file or folder is
made. It answers the consent and a `preview_key`, a random nonce the viewer keeps with the
fingerprint of what it previewed (config.toml's version, the choice and, for `local`, each file's
name, size and pin), as the recovery preview does (`settings/recovery.rs`):

- `none`: nothing leaves the machine; the vectors stay in the store; the local model's files stay
  in their folder (its size), which the page does not delete.
- `workers-ai`: each memory's text after redaction and each search query go to Cloudflare
  Workers AI under the saved account, within the saved caps. Without an account or a readable
  token it is refused (`no_account`, `no_key`) and the page points at those fields.
- `local`: refused where the build or machine cannot run it. Otherwise the hosts and sizes still
  to download (huggingface.co for BAAI's bge-m3, github.com for Microsoft's ONNX Runtime, both
  MIT), or a check of the files already there, and that afterwards no text or query leaves the
  machine to be embedded.

`POST /api/embedding {"choice", "preview_key", "confirmed": true}` consumes the nonce once and
compares a fresh fingerprint (a config or files changed since are `stale`). The viewer runs one
choice at a time and keeps the last one's result, with a guard that records a panic as a failure,
as maintenance runs do (`settings/maintenance.rs`):

- for `local`, it downloads what is missing and checks every file against its pin
  (`model_fetch::fetch`, `fetch_runtime`, `verify`), holding only the model folder's lock, so a
  `setup --embeddings` running at the same time refuses one of them (`busy`) and every other
  settings save goes on meanwhile. The request lasts the download; the page reads the progress
  from `GET /api/settings` (the bytes held grow as the `.part` files do). An interrupted or failed
  download changes no setting: the files stay, the state says `incomplete`, and a new choice
  resumes it;
- then, and only then holding the settings lock and config.lock, it writes `[embedding]
  provider` as `settings::set_embedding_provider` does (every reader's parse, version check).

The viewer's guards apply to both: token, Host, Origin, JSON body, body cap.

## Workers AI's account, caps and token

`account_id`, `daily_requests` and `monthly_usd` join the page's settings save
(`Save.embedding`), checked as the page checks its other values: an account id is Cloudflare's
32 hexadecimal characters, `daily_requests` 1 to 100,000 (the provider budget's range), and
`monthly_usd` a finite number above 0, as config.toml already requires. While `workers-ai` is
the choice, the page shows the destination line next to the account field, so a changed account
is seen before it is saved.

The token is write-only: `POST /api/embedding/key {"key", "version"}`, under the same guards
and the same 1 KiB body cap as `/api/providers/key`, registers it as W2 registers a provider's key
(`keyfile::managed`: a new owner-only file outside the store) and points `[embedding] key_file`
at it. Answers carry the key state only. Registration is Linux only until #281, as for providers:
on macOS and Windows the page shows the key state without a form (the iMac, the one Workers AI
machine, keeps its token in the default `key_file`, which shows `ok`). This endpoint is security
scope: its PR runs semgrep and a security review.

## The index

Workers AI and the local runner share one generation, `bge-m3` (milestone 4 D8), so switching
between them keeps every vector, and switching to `none` keeps them for a later choice. The page
says so, and shows the waiting counts the embedding phase is working through. No choice offered
here builds a new generation; one would come only with another model (#396), which W4 does not
offer.

## Not in W4

Deleting the local model's files from the page; the CLI's `none` line and the page both name the
folder and its size. Per-prompt injection of past work by meaning (G23) is its own change.

## Tests

- `show`: the `embedding` section for a fresh home, a Workers AI home with and without its token,
  and each local state (unavailable, not downloaded, incomplete, changed, ready), with no key in
  any answer.
- preview and choice: the guards; a preview sends no request and makes no file or folder; a
  nonce used twice or after a config edit is `stale`; a refusal (no account, no token,
  unavailable) leaves config.toml's bytes as they were; `local` against the stub host downloads,
  verifies and then writes the provider, a failure writes nothing and reports its code, and a
  held model lock answers `busy` with config.toml's bytes unchanged; the bytes held are visible
  while the download runs, and a settings save goes through meanwhile.
- `setup --embeddings` prints the same consent as before (`setup_embeddings` unchanged).
- the save: `embedding` values checked at their ranges, comments kept; the key endpoint writes a
  managed file and the reference, answering no key.
- the page: Japanese and English strings for every state and refusal (`app.js` checks).
