# Clef / Clef-flash: where it could be used, and the "hooks only read" rule (2026-10-03, Claude)

Source: a study of 2026-10-03 (28 readers: facts with sources, 40 ideas, 8 picks each given to a skeptic). Its full
result is kept outside the repository (`../oboete-work/takeover-2026-10-02/clef-study-result.json`).
The owner asked (2026-10-03): 「判断は任せる。使い所が他にないかも検討して。あと、フックに入れない決まりに
重大な理由がないならそこも見直して。それと、clef-flashは量子化なら全然低vramでも動くよ」.

## 1. Jev and Clef in the spec

- Jev was never scheduled. Spec 3.5 names it as one of four judge candidates, both judge roles are
  "enabled only if evaluation shows a gain", and no judge code exists in `src`.
- Recommendation: spec 3.5 names "a decision model (Clef-flash, Clef; any System One compatible
  endpoint)" in place of "Jev". Reasons: open Apache-2.0 weights (Jev is closed and its sign-ups are
  paused), the same Cloudflare account and free 10,000 neurons a day that the embeddings use (about
  1.22M Clef-flash input tokens or 458k Clef input tokens a day, by arithmetic from the price
  page), identical answers on identical requests in the one independent test (Jev moved up to 0.12).
- This is a wording change. Nothing is built and nothing is switched on by it.

## 2. Use sites: 40 ideas, 8 picked, none survived the "gain" check

Every pick was refuted on gain by its skeptic; seven of eight also on cost or risk.

| Pick | What it was | Why it fell |
|---|---|---|
| P1 | classify the owner's reply (accepts / rejects / asks) for the promotion gate | of the 6 labelled acceptances, 5 already pass the keyword checks or are not acceptances; the blockers are drafting, taint and quote location, which the model does not touch |
| P2 | second opinion on "this decision replaces that one" | a veto only makes links stricter and cannot fix the larger half (the link was never drafted); a code rule (links end a decision only inside one session) removes most wrong links at no cost |
| P3 | re-anchor paraphrased drafts on the owner's line | the 195 dropped drafts are not tied to the missed labels; two of the three cited numbers were already fixed (#248, #334) |
| P4 | detect an owner decision the curator missed and curate that window again | only about 15 of 40 typed misses have no draft; the last test of a second pass gained too little |
| P5 | grader for the evaluation panels | the panels are already non-Claude models; only agreement with Sonnet would be measured |
| P6 | worker-stored scores per claim, read by the hook | the worker cannot see the prompt that decides relevance; the smaller form is alias text per claim (below) |
| P7 | skip curator calls for windows with no owner line | it is a code rule and needs no model; as written it would stop `done` closures from passing runs, so it needs its own design |
| P8 | who-said-it re-check | the measured case went to 0 with #275; the rest is unmeasured |

What nobody has measured anywhere: Clef on Japanese text. No statement by Cloudflare, no Japanese
benchmark, no third-party test. The nearest independent test (nicia-ai/admission-decision-eval, 85
cases, English) has Clef (27B) at 0.97 on harmless writes and Clef-flash (9B) at 0.66, with 19 of 85
Clef-flash answers in the unsure band.

One probe is worth its cost when time allows: the owner-confirmed dev pairs (20 overturns, 26
compatible), each asked in both orders, both models. About 1,400 neurons (14% of one free day), no
Claude use. Pass rule fixed before the run: wrong "overturns" on compatible pairs at most 1 of 26,
at least 14 of 20 overturns recognised, at least 44 of 46 answers the same in both orders. A fail
ends Clef for oboete; a pass only makes it a candidate for the single final evaluation.

Found on the way, no model needed (candidates for after the switch):
- links end a decision only when both ends share a session (P2's verdict);
- claim-side alias text ("questions this claim answers"), written by the curator in the worker when
  the shortlist is built, matched by the hook with the trigram share it already uses (the hook rule's
  simplicity verdict): aimed at "8 of 22 answerable prompts served";
- skipping windows with no owner words (P7), after counting them on the stored arm homes.

## 3. The "hooks only read" rule (spec 4.1 to 4.3)

Reasons that are serious:
- A hosted call from a hook sends every typed prompt off the machine at every submit, and the prompt
  hook has no exclusion check of its own (exclusion is applied in the worker).
- Time: the whole read hook takes about 13 to 40 ms at p95 (docs/spike/read-hook.md); hosted Clef-flash measured independently is
  430 to 530 ms at p50, 0.7 to 1.9 s at p95, with calls of 9 to 26 s and 429s at launch. Every prompt
  would wait 10 to 50 times longer, and offline it fails.
- Capture hooks must never wait on anything (a 20 ms line that macOS and Windows already strain).
- A hook process cannot load a model itself (bge-m3: 2.1 to 2.5 s and 1.79 GB per process).

Reasons that no longer hold:
- "Nothing runs between hooks": the owner made the worker resident (decision 36).
- Agent hook timeouts and the old 300 ms budget: the spec already dropped them.

Recommendation: keep the rule and say it in three parts.
1. Never: a hook calling anything off the machine; a hook loading a model; any call at all from the
   capture hooks and SessionStart.
2. Allowed: hooks read what the resident worker stored, model-made scores and alias text included.
3. Not now: the prompt hook asking the resident worker (local model, same machine). It needs a
   listener (security scope), per-OS pipes, a fallback, and today it would work only on the RTX 5080
   PC at roughly four times the hook's time. Reopen it only if a measured gap is left after part 2.

## 4. Running it locally

The owner is right that quantized Clef-flash runs in little VRAM: third parties ran FP8 (12.2 GB)
and NVFP4 (9.2 GB) on an RTX 5070 Ti 16 GB and a 4-bit EXL3 build (8.4 GB) on an RTX 4090; MLX
4-bit peaked at 5.9 GB on an M2 with 24 GB (2.9 s an item). Limits today: all of these are
third-party builds (vLLM plugin in Docker, ExLlamaV3, MLX); the common GGUF files leave out the
decision head; llama.cpp's support is a draft PR (#29831); Ollama merged support on 2026-10-01 but
only a pre-release has it, untested here; the 8 GB iMac is out; Clef (27B) does not fit 16 GB.
So: the hosted API first; local becomes a `base_url` setting once Ollama or llama.cpp ship it,
because llama.cpp's `/v1/systemone` route (merged 2026-10-02) is the same request shape.

## 5. Order

Nothing here comes before the PC switch. The spec wording of sections 1 and 3 is owner decision 38,
in the docs PR that brings this study. The probe of section 2 runs when it does not delay the
switch.
