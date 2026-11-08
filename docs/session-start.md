# SessionStart: what it gives less of (decisions 75 and 76, two faults)

A survey on 2026-11-07 read what this PC's SessionStart gives an agent in this repository (`oboete inject`, knowledge.db's stored manifests). It found:

- **The index takes the most room.** "More from memory" was 22 lines, 2,322 of 8,393 characters (28%), its 20 claims one line each; the recent-context block showed 25 cards, halved from 50 because 50 did not fit.
- **Old open items stay in "Decisions and open items".** Of the 10 bodies there, 4 were open items whose work had finished, and one had been replaced; nothing ages a claim out of a body.
- **"Last exchange" shows another program's prompt.** Of 60 stored manifests, 37 showed as the last prompt a prompt another agent sent or Codex's review template. `facts` records every prompt as the checkout's last, whoever sent it (`consumer/manifest.rs`).
- **Codex's review template reads as the owner's words.** Codex's built-in review prompts ("Review the code changes against the base branch '…'. … Provide prioritized, actionable findings.") carry no `agent_sent` mark: 22 live records in raw.db. The curator labels a prompt `[user]` unless it is marked, so three of the delivered decisions stood on such a prompt.

Decisions 75 and 76 (2026-11-07) settle the first two; the last two are faults, fixed here.

## Plan

### 1. Codex's review prompts are marked as sent by a program

`typed::codex_review_preset(prompt)` is true for the five review prompts Codex 0.166.0 builds (read from its binary): the current changes ("Review the current code changes (staged, unstaged, and untracked files) and provide prioritized findings."), a base branch with its merge base, a base branch with its upstream, and a commit with or without its title, the last four opening "Review the code changes " and closing "Provide prioritized, actionable findings.". The test is the whole text: the branch, merge base, commit and title are read from the prompt, the template is rendered with them, and the two must be equal. `codex review "<text>"` sends the owner's own text as typed, and stays the owner's even where it opens and closes as a template does.

- **At capture** (`capture.rs`, where a payload's `agent_sent` reaches the body), a prompt the test takes is marked `agent_sent`, from any agent and any source (a hook or an import): so curation reads it as `[agent prompt]`, the manifest's owner-line check skips it, and My Work's `typed` leaves it out.
- **At read**, for the records already stored without the mark: My Work's `agent_written` takes it beside `assigns_a_role`; the curator labels it `[agent prompt]` when a backlog or a recuration reads it; the manifest's build skips it among the owner's directive lines, whose facts were recorded before; and "Last exchange" (2) applies it.
- Not changed: the 22 records stay as they are (raw.db is append-only), and the claims already curated from them stay delivered until the owner forgets or mutes them, or decision 76 folds them.

### 2. "Last exchange" is the owner's last typed prompt and its reply

`build` walks the checkout's prompt facts newest first, at most 200 (`shortcut: a checkout whose last 200 prompts were all a program's shows no last prompt`), and takes the first whose record `typed::typed` passes (no `agent_sent`, and for Claude Code a typed entry point) and that neither `assigns_a_role` nor `codex_review_preset` takes. The reply shown is its turn's: the first reply fact of that prompt's session in the repository after it, before the session's next prompt in any repository on the device (any prompt, a program's too; #856), on whatever branch the session had moved to, and by the prompt's agent, or none. Not another repository's: a record there rebuilds, and a forget there reaches, that repository's checkouts only, so another's text would stay. The bound needs no rebuild of its own: a reply it cuts off is a record of the repository shown, which rebuilds its checkouts, and a forget of the other repository's prompt can only leave this one showing a reply of its own. Two indexes make each lookup a seek: `manifest_facts_turn` the reply, `manifest_facts_session` the session's next prompt. So the section never pairs the owner's prompt with the answer to a program's, in its session or another. The facts stay as they are: no rebuild of `manifest_facts`. With #854 `FORMAT` went from `m4` to `m5`, so every text stored before it was built again with that choice. #856 leaves it at `m5`: a text built before #856 keeps its pairing until its checkout's next record.

### 3. Decision 76: a decision or an open item older than 14 days gets no body

In `delivered`, a unit (a claim with the later claims that ended it, `claims::units`) whose newest claim is a `decision` or an `open item` with `valid_from` more than 14 days before `now` takes no body place: it joins the index's candidates where its rank put it, so it is an index line while the index has room for it, and is otherwise left to search. Only those two kinds: a lesson or a preference is not finished work, and MUST-M21's lesson about the checkout's failing command keeps its body at any age. A fresh unit that holds a superseded claim is shown as before. `now` reaches `delivered` from `text`.

### 4. Decision 75: the index shows `[inject] session_start_index` lines, 10 by default

`INDEX` becomes `[inject] session_start_index` (0 to 50, default 10), passed to `manifest::text` beside the size. The settings page shows it with the session start's size, in English and Japanese. The cards take the freed room: the loop that fits them (docs/cards.md S1, docs/summaries.md S9) halves them until the packet fits, as claude-mem does, which would leave the room unused when 50 cards are just too many; so once only the cards are left to halve, it then takes back as many of the newest as fit below the count that did not, tried from the largest down: a block's size need not grow with its cards, as it drops its savings line once the cards shown cost as much to read as the work recorded (at most 24 more packets when 50 fell to 25).

Measured here with `oboete inject` (2026-11-07): the index went from 22 lines and 2,322 characters to 12 lines and 1,256; the cards from 25 to 49.

## Acceptance tests

1. Each of the five templates is a template; free text, text that opens and closes as a template with another middle, a template with words appended, and a merge base that differs from the one its `git diff` names are not. A template prompt is stored with `agent_sent`, from any agent and with prompts kept or not; the owner's own review request is not.
2. The curator labels an unmarked template prompt `[agent prompt]`, and the owner's own review request `[user]`.
3. With an owner's prompt and its reply, then in its session a second reply, an agent-sent prompt and its reply, and in another session a template prompt and its reply, "Last exchange" shows the owner's prompt and the first reply of its turn, never a reply before it or the programs' replies; an unanswered later prompt of the owner's shows alone, as one whose session's next reply is in another repository or another agent's.
4. With only programs' prompts, "Last exchange" is not shown, and a template whose commit title says "Always …" is no directive of the owner's.
5. A stored manifest of format `m4` is built again.
6. With `now` fixed: a decision of 15 days ago shows as an index line, not a body; one of 14 days exactly keeps its body; a lesson of 30 days ago keeps its body; an open item of 15 days ago is an index line; a pair whose later claim is a day old keeps both bodies, and a pair whose later claim is 20 days old is two index lines.
7. With 15 claims, the default index shows 5 lines and 3 shows 3; 0 shows none, with the 10 bodies and the tools line kept; the default is 10.
8. The settings page reads and saves `session_start_index`, refusing a value outside 0 to 50.
9. `the_manifest_is_the_same_bytes_for_the_same_records` stays true with `now` fixed. The tests' read time is 14 days after the epoch (it was a year), so their claims keep their bodies; the hooks' tests, whose claims are dated in 1970 and which read at the clock's time, set `FOLD_AT`, a test-only reference time from which decision 76 measures a claim's age, to 14 days after the epoch, so their claims dated in 1970 are not older than 14 days.
10. One character short of the room for 8 cards, 7 are shown (halved to 4, then 3 taken back), the newest; with no room for one, none. With block sizes that do not grow with the count (4: 462, 5: 485, 6: 507, 7: 470, 8: 494) and room for 470, 7 are taken back.

## Not in this change

- Counting the per-prompt block's injections and their size in `oboete doctor`.
- The parity table's reading of claude-mem 15.10 and 15.11 (rows X20, X22, 41, 33, 58, and decision 77's row).
- Shorter MCP tool descriptions.
- The curator's label for a Claude Code prompt from `claude -p` (`entrypoint` `sdk-cli`), which `typed` leaves out but the curator reads as `[user]` (since done: the curator labels a Claude Code prompt from an entry point no person types in `[agent prompt]`).
