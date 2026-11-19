# Spike: claude-mem's work-view on oboete's engine (decision 114)

Throwaway (spec 8.3). Decision 114 (2026-11-17) serves claude-mem's work-view on oboete's engine, after a throwaway spike in the isolated user ([page.md](../page.md), "claude-mem's My Work pages"). If the spike failed, oboete's own page would be extended by hand instead. Run on 2026-11-18, on `main` at `a986762a` and claude-mem `v15.19.0` (`01a86b23`).

**Result: pass.** The real pull requests can start; "Next" below lists them.

## Question

Can claude-mem 15.19.0's work-view run on oboete's engine? The work-view is My Work, focus areas, Suggested, Past focus and My Rules. Running on the engine means three things:

- oboete's viewer serves the page;
- the page reads oboete's data through one adapter endpoint;
- claude-mem's mapper and screens stay unpatched.

It passes only if all seven items under "Results" hold. Any one of these is a fail:

- the mapper or a screen needs a patch;
- a screen needs data the engine does not hold, so the engine would need a new reader. Reshaping existing answers is fine;
- polling cannot keep the connection badge at "Live";
- the page needs `'unsafe-inline'` or `'unsafe-eval'` in its CSP;
- two working days pass without a pass.

## Harness

`docs/spike/work-view/`. The npm build, the viewer, its homes and the Node check ran as the isolated user `oboete-dogfood`, on throwaway homes, with no model call. Three things ran as the owner's account: the build of oboete, the copy of claude-mem's sources, and the browser, saving and timing scripts. Those scripts talked only to the spike's viewer on port 17381. No store of the owner's was opened. Every token was read inside a script and never printed; a JSON report replaces it with `<t>`.

- **The page.** The work-view sources were taken from the tag. claude-mem's `src/` layout was kept, with the three `.d.mts` type files and the root `package.json`. Two files changed:
  - `local-api-token-client.ts` was replaced by `local-api-token-client.ts` here (P1). It reads the token from `#t=`, sends `X-Oboete-Token`, refuses any other origin, answers the usage and activity reports itself, and polls `/api/work-view/version` in place of the event stream. The poll runs every 3 s and re-reads the snapshot when `v` changes and every 60 s.
  - `vite.config.ts`'s `outDir` became `dist` (P2).

  It was built with `npm ci --ignore-scripts` and `npm run build`.
- **The server.** `server.diff` is the spike branch, against `a986762a`. It adds four things:
  - a `/work-view/` arm that serves the bundle from `OBOETE_WORK_VIEW_DIR`, with no token, as `/app.js` is served;
  - a CSP for those files that adds `font-src 'self'` and `img-src 'self' data:`;
  - `GET /api/work-view/version`, the same answer as `/api/version`;
  - `GET /api/work-view`, the adapter, `src/work_view.rs`. It reshapes the gated answers of `/api/my-work` (`range=week`) and `/api/my-goals` and reads no store itself.

  `OBOETE_WORK_VIEW_FIXTURE` (a snapshot to answer as it is) and `OBOETE_WORK_VIEW_INPUT` (a hand-written My Work answer to run the adapter on) were for the spike only.
- **The homes.** The small home holds the made-up events of `synth.py` (130 events, 2 requests). The large home holds the fixture of record, `events-1000.jsonl`, whose sha256 `9af34aad…` equals its copy in `oboete-ts`. `oboete replay` replays only its 255 Claude Code lines unless given `--agent all` (`main.rs:495`, `replay.rs:78-86`), and they give 11 requests. Neither home has focus areas, goals or rules: those come from a curator's ops, and no model was called.
- **The checks.**
  - `drive.mjs` opens screens in headless Chromium and records the console, page errors, CSP violations and requests.
  - `live.mjs` watches the connection badge. It can stop the viewer with `SIGSTOP` for 40 s, and it counts the snapshot reads and any request URL that holds the token.
  - `get.sh` saves answers.
  - `compare.py` checks a snapshot against `/api/my-work`.
  - `spike-check.ts` runs claude-mem's own `toWorkModel` in Node over a snapshot.
  - `timing.sh` times the three answers.
  - `licences.mjs` lists the packages in the bundle from the source maps of a second build, made with `npx vite build --sourcemap --outDir dist-map`.
  - `synth.py` writes the small home's events.
- **The inputs.**
  - `contract-min.json` is a minimal snapshot written by hand from the mapper's code.
  - The hand-written My Work answer had three areas: a kept one with quotes and an ask, a suggested one and a dismissed one. It had one other ask. It had 13 requests covering all 9 of oboete's statuses, including `ready` with a check and `shipped` three ways.

## Results

| # | Item | Result |
|---|---|---|
| 1 | The build | **Pass.** It took 9 s. Upstream changed in the two files above and nowhere else (`diff -r` against a clean extract). |
| 2 | The screens on a fixture | **Pass.** These rendered: My Work, a focus area, Suggested, Past focus, My Rules and the request drawer. There was no read-error card, no goals or rules error, and no uncaught exception. |
| 3 | The badge | **Pass.** "Live updates" showed within 5 to 7 s. It stayed for 2 min 13 s. A 40 s `SIGSTOP` of the viewer turned it into "Local feed stale" 37 s after the stop. It was back within 5 s of `SIGCONT`. |
| 4 | Real data | **Pass.** On both homes, `/api/work-view` showed the requests of `/api/my-work?range=week` (2 and 11). The count, each title and each status, mapped as below, were the same. There were 0 read-error cards. Every conversation found its agent. |
| 5 | Focus mapping | **Pass.** The hand-written answer went through the adapter and then through `toWorkModel` unchanged. The kept area had 4 times asked, its latest stamp, 2 quotes and 1 ask, as written. The other ask came out as written too. There was no goals error. Each status mapped as below. |
| 6 | The network | **Pass.** No request left the viewer's origin. P1 refused the GitHub stars call before it was sent. Over 130 s, no request URL held the token. |
| 7 | The snapshot's cost | **Pass.** These were measured on the large home with a debug build, 11 requests, median of 10: `/api/work-view` took 44.1 ms, against 40.1 ms for `/api/my-work?range=week` plus 5.5 ms for `/api/my-goals`. A home the owner's size was not measured; its cost is `listed`'s (#1037). |

How each oboete status shows. The adapter sends it as an explicit lifecycle, which the page takes over its own guess:

| oboete | Shown as |
|---|---|
| `needs`, `blocked` | Your move, Building |
| `ready` | Done, not checked; with a check, upstream's `checked`: Your move, "Ship it" |
| `working` | Working, Building |
| `todo` | Working, Planning |
| `quiet`, `closed` | Idle |
| `unknown` | Working when updated in the last 6 h, else Idle |
| `shipped` | Shipped, with or without a percent and a completion time |

## Measurements

- **The bundle.** It has 39 files and 2,425,438 bytes:
  - 18 JS files (1,584,919 bytes; 515,980 with gzip -9);
  - 2 CSS files (185,978);
  - 11 focus illustrations (434,666);
  - 6 fonts (209,312).

  Embedded with `include_bytes!`, it would add about 2.4 MB to a 30 MB release binary. That figure is not checked with a build.
- **The snapshot's size.** It was 6,100 bytes against My Work's 5,197 on the small home, and 21,081 against 15,584 on the large one.
- **The load.** One open page asks `/api/work-view/version` 20 times a minute and reads the snapshot about once a minute.
- **Expected failures.** `GET /api/setup/state` answers 404, which keeps claude-mem's setup shut. `GET /api/work-view/focus/shares` answers 404, which the page ignores.
- **Without a token.** The page sent three requests, each answered 401. Then it showed P1's notice and sent nothing more.

## Findings for the real pull requests

- **The build environment.** claude-mem's repository root provides two things the build needs:
  - its `package.json` (`"type": "module"`): without it, `tsc` reads `src/ui/viewer/utils/*.ts` as CommonJS (TS1287);
  - a `node_modules` that the files outside `src/ui/my-work` reach: without it, `src/ui/i18n` cannot find `react` (TS2307).

  The vendored tree takes the root `package.json`, and the build links the root's `node_modules` to `src/ui/my-work/node_modules`.
- **The CSP.** oboete's present policy blocks only Geist's five `.woff2` files and the `data:` grain images. `font-src 'self'` and `img-src 'self' data:` cover both, and scripts need nothing more.
  - The request drawer adds two `<style>` elements (`react-remove-scroll`), which `style-src 'self'` refuses. The drawer still renders and works; most likely only the scroll lock behind it is lost.
  - A nonce (`get-nonce`) is the thing to try; `'unsafe-inline'` stays out.
- **The agents.** An agent's `platformSource` must be the platform inside each conversation key, because the drawer finds a conversation's agent by the pair (`to-work-model.ts:226`). The adapter sends oboete's agent name, with `agy` as `gemini`. claude-mem marks `claude`, `codex` and `gemini` and shows the rest as "Agent".
- **Sync.** With no `sync` block, the header says "Cloud freshness unknown". A valid unconfigured block makes it "Cloud sync off". Hiding the label belongs to the patches.
- **Still claude-mem's, unpatched in the spike.** The brand and mark, "Team", the machine count, Classic viewer, Feedback, Star, the language list with no Japanese, and the rules-file card. The English catalog needs an override for the brand, because its keys are the English text.
- **Licences.** Decision 114 asks for each file in the bundle to be checked for its licence in the spike. Each one has a confirmed licence, so none needs replacing (`licences.json`).
  - The source maps name 105 npm packages. 100 declare MIT. The others are `tslib` (0BSD), `class-variance-authority` (Apache-2.0), `ansi-to-react` (BSD-3-Clause) and `lucide-react` (ISC). `@tsparticles/react` declares none, but its `LICENSE` is MIT.
  - `ansi-to-react` and `react-remove-scroll-bar` ship no licence file. `react-remove-scroll-bar`'s text comes from its repository. `ansi-to-react`'s repository has none either, so its text is the BSD-3-Clause licence with a copyright line made from its package's metadata, which names its author (`ui/work-view/licences/`). The rest of the JS is claude-mem's own code, Apache-2.0.
  - Geist and Hanken Grotesk are OFL-1.1. Hanken Grotesk is used only by claude-mem's setup page, which oboete hides, so dropping that page also drops its 133 KB.
  - The focus illustrations came in with claude-mem's commit `45a0dec2` (2026-11-09), by the repository's owner, and no other commit touched them. So the repository's Apache-2.0 covers them.
  - `favicon.svg` is claude-mem's mark, which the brand patch replaces.
  - What decision 114 asks before each release is still to come: where each package comes from and its advisories, such as `linkify-it` 3.0.3 through `ansi-to-react`.
- **The adapter drops an area's asks when the area has no priorities.** oboete's `/api/my-work` gives such an area `asked: null` but still lists its asks (`my_work_focus.rs:336-340`). The spike's adapter skipped the whole area. The read adapter takes an area's asks whatever its count, and its contract check covers an area with asks and no priorities.
- **Two comments in `src/view.rs` are out of date.** The route's "Reads only" (lines 2472-2473) is one: the viewer takes writes. The review's doc comment (2219-2222) is the other: it sits above `my_work_refresh`, not `my_work_review`. The pull request that changes the routes fixes both.

## What the pass does not cover

- **The address and token of their own.** The page's calls go to `/api/work-view` and the paths under `/api/work-view/`, with one exception: claude-mem's setup probe, `GET /api/setup/state`. It must keep answering 404, which keeps the setup page shut. A token that reaches only the work-view's paths would answer it 401, and P1 stops the whole page at a 401. So the scoped token needs three changes: the server's, P1's storage key, and P1 answering the probe itself. It is a pull request of its own, in security scope.
- **Writes** (review, mark finished, refresh, focus actions, suggestions), **My Rules' rules**, **Japanese**, and the **adapter fields left out** (subagents, work products, goal history, sources, continuation).
- **Focus data made by oboete's own engine.** The focus screens were fed only the fixture and the hand-written answer. They are checked on the real screen when the read adapter lands.

## Next

These are the study's pull requests, corrected by this spike and by their reviews. The study of serving claude-mem's web UI is kept outside the repository.

1. **Vendor** the pristine tree at the tag: the files above with the root `package.json` and the three `.d.mts`, and `NOTICE` (#1080). Sonar skips the vendored tree and the bundle; Codacy's settings are #1081.
2. **Patches and build**: P1 and the hiding patches, `build.sh`, the committed bundle and the Node job (#1082). Nothing is served.
3. **The read adapter**: `GET /api/work-view`, from `src/work_view.rs` here, with a contract test that runs `toWorkModel` over its answer. It answers at the main page's address and with its token, as `/api/my-work` does, until serving gives it its own.
4. **Serving, at an address of its own** (done: the bundle's side in #1099, then the viewer's, at `work-view.localhost`, decision 116): the `/work-view/` arm with its CSP, the address of its own, its token, and the two comments above. Serving waits for the address: the main page keeps its token in its origin's storage, which the bundle would share at the same address.
5. Then the writes, the rules and Japanese, one pull request each.
