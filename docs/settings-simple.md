# A simple first view of the settings

Plan of the slice, written before the code, as the other slices' notes are. Decision 45 (2026-10-14)
puts the settings page, checked on the real screen for how easily its model and feature settings are
understood, before the switch, with what that check finds fixed. The check ran on `main` 02634d1, on
a fresh home of the isolated user, in Japanese. The page offers nearly every setting, but it does not
read easily:

- expanded, it is about 22,600 px tall at 1280 px wide, with some 17,000 characters of text;
- nearly every control carries a safety caveat ("saving starts no call", "the test follows no
  redirect", a field's whole `u32` range);
- the nine built-in summary providers are listed alike, each with a full editor (URL, API, a free
  text model, timeout, token caps, prices), and none says on its row whether it can be used: a
  missing key or a CLI that is not installed is shown in a second table further down;
- that second table has its own arrows, and its order overrides the first one's;
- three ways of saving sit side by side: the page's save, a provider's save, and controls that save
  at once;
- internal counters and process notes show ("check position: 0", "off until measured");
- "repositories not sent out" and "never recorded" read alike;
- seven agents are listed alike, four of them not used at the switch;
- the first-run tier is applied to the unsaved settings and then saved, two steps.

So the settings tab opens on a simple view of the settings most homes need, and the current page
stays whole as the detailed settings, one click away. Nothing a setting does changes; what changes is
what is shown first and how it is said.

## What the simple view holds

1. **AI for summaries.** One list, in the order the chain uses (`settings::show`'s `chain`, one row
   per name; its arrows set the name order, the only order this view shows). Each row has:
   - a label a person reads, from the catalog below, and the model in use (`effective_model`);
   - a status from the row: `key: "missing"` needs a key; `key: "not-on-path"` is not installed; a
     name in `stopped` is stopped, with the existing resume button; `on: false` is off; otherwise it
     is ready;
   - an on/off switch and ↑ ↓, which save at once, as the current controls do;
   - when the key is missing, the existing key field and its save, inline;
   - the existing connection test (its preview, then the test);
   - a link to the row's entry in the detailed settings.

   The model is a select of the catalog's models for that provider, the one in use first. A row
   whose `model_rule` is `free` lists only models whose name ends in `:free`; a row whose rule is
   `fixed` (a priced entry: its model and price change together) shows the model as text with a link
   to the detailed settings. "Another model" opens the entry in the detailed settings. A name whose
   entries differ (`differs` not empty) says so and links there too.

   **Add AI** opens a short catalog. A provider already in the list says so (it is turned on there).
   Choosing one fills the existing add form with its name, URL, API, key file, subscription flag
   and first model, and the key step follows; the save is the existing add call.
2. **Monthly limit on paid calls.** The dollar field with this month's spend beside it.
3. **Search by meaning.** The three choices, a line each. Choosing this computer or Workers AI opens
   the existing review (what is downloaded, or what leaves the machine) before anything is saved.
4. **What agents are given.** Memory at the start of a session, on or off; related decisions at each
   prompt, on or off. Sizes stay in the detailed settings.
5. **Places never recorded.** The repositories and this device's folders, each with its remove, and
   a field to add a folder: the existing calls.
6. **Masking of secrets.** On or off, with the existing warning before off.
7. **Agents.** The three the switch connects (Claude Code, Codex, Grok), each connected or not, read
   from the existing inventory, with a link to connect them in the detailed settings.

A home where no provider is ready shows one line at the top: no AI for summaries can be used yet;
add a key in the list or add an AI. The tier select stays in the detailed settings.

## Saving

Every control of the simple view saves when it changes (switches, selects, arrows) or when its field
is left or entered (the limit, a folder), and says "saved" beside it. What sends or deletes something
keeps its existing confirmation: a connection test, the embedding review, masking off, forgetting.
One line under the view's title says so. The detailed settings keep their own save.

## Words

One line under each block says what it does for the user. Where money is spent or something leaves
the machine, it is said once, there. No field ranges, internal counters or process notes. Japanese
first; English says the same.

## The catalog

A table in the page (`assets/viewer/app.js`), with no new backend: per provider its label in both
languages, its kind (an HTTP API or a CLI), URL, API, key file name, whether it is a subscription,
and its models, recommended first. The providers and models are the ones oboete already ships
(`provider.rs`'s built-in entries) and the ones its research notes name
(`docs/research/curator-providers-*.md`, `docs/research/anthropic-messages-2026-10-09.md`), plus
another OpenAI-compatible server with no preset. A list of models goes stale, so it is a help, not a
check: any model is still set in the detailed settings.

## Detailed settings

The current page, unchanged, under a "detailed settings" switch at the top of the tab. The browser
remembers which of the two was open last.

## Tests

- The pure parts (a row's status, the models a row may pick, the catalog's "already in the list")
  are functions the Node harness (`src/testdata/viewer-readiness/test.mjs`) runs, as the readiness
  checks are.
- A browser check by hand (Playwright's Chromium, a home of the isolated user, nothing of the owner's
  read or written), in Japanese and English, at 1280 and 390 px: the first screen without a ready
  provider, a key added and the row turning ready, the order changed, a model picked, search by
  meaning's review opened and cancelled, a folder added to the never-recorded list and removed,
  masking turned off with its warning and on again.
