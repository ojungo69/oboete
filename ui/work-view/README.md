# claude-mem's work-view

Decision 114 (2026-11-17) serves claude-mem's work-view on oboete's engine: My Work, focus areas, Suggested, Past focus and My Rules ([page.md](../../docs/page.md), "claude-mem's My Work pages"). The spike that came first is [spike/work-view.md](../../docs/spike/work-view.md).

- `upstream/` is the work-view as claude-mem published it, unchanged. It holds the sources, the files outside `src/ui/my-work` they import, and claude-mem's root `package.json`, which sets `"type": "module"`. `LICENSE` and `NOTICE` are claude-mem's. claude-mem's `src/` layout is kept, because the sources' relative imports climb to it.
- `UPSTREAM` names the tag and the commit `upstream/` came from. oboete's `NOTICE` points here rather than repeating them.
- `fetch.sh` makes `upstream/` again from a claude-mem checkout at a tag: `ui/work-view/fetch.sh <checkout> <tag>`. Nothing else changes `upstream/`. A Dependabot pull request that edits its `package-lock.json` is closed, and its advisory is handled where the bundle's packages are checked (step 4 below).
- `patches/` holds oboete's changes, applied in order. Each file they change says "Modified by oboete".
  - `0001-token-and-transport.patch` sends oboete's token header, keeps the pages' own key in the browser, and polls in place of claude-mem's event stream. It answers claude-mem's setup probe in the page, and takes the key out of the address once read.
  - `0002-build-output.patch` keeps the build inside its copy.
  - `0003-hide-what-oboete-lacks.patch` hides what oboete does not have yet: continuing a conversation, sharing an area, usage reports, the team view, the machines and the specification overlay.
  - `0004-oboete-brand.patch` puts in oboete's mark, name, title, favicon and Japanese font fallback, and hides claude-mem's own links.
- `build.sh` builds the bundle into `assets/work-view/`:
  - it applies the patches to a copy;
  - it installs the packages with `npm ci --ignore-scripts`;
  - it type-checks and builds;
  - it writes `THIRD-PARTY-NOTICES.md` with `notices.mjs`, which lists every package and font the bundle holds with its licence. `licences/` holds the texts of the packages that ship none, and the build stops at a package with no text.

  Run it as a user who cannot read your keys: the build's tools run native code even with install scripts off.
- `contract/` holds the contract between oboete's answer and the pages: `snapshot.json`, the answer of `GET /api/work-view` (`src/work_view.rs`) for a synthetic home, which the Rust test `work_view::tests::the_contract_snapshot_is_this_homes_answer` keeps current (it compares the two with times and keyed prints written as their kind; with `OBOETE_CONTRACT_WRITE=1` it writes this run's answer, whatever it is. In the first 15 minutes of a month, when the home's newest goals fall in the month before, it refuses to write and does not compare), `stories.json`, the Build story the adapter writes for each case of `STORIES` in the same Rust file (its test `the_contract_stories_are_the_adapters_answer` keeps it current the same way), and a Node test that runs claude-mem's mapper, unchanged, over all three (`node --test 'ui/work-view/contract/*.test.mjs'`, which the workflow runs again with `TZ=America/Los_Angeles`, as the pages run in the browser's zone). A bump of claude-mem runs the new mapper over the same answer.
- `notices.test.mjs` runs `notices.mjs` over small built trees it makes (`node --test 'ui/work-view/*.test.mjs'`).
- `.github/workflows/work-view.yml` runs `build.sh` again in CI. It fails when the bundle it makes differs from the committed one by a byte.

The bundle is not served yet. Its data is `GET /api/work-view` (`src/work_view.rs`), which answers at the main page's address with its token; the pages' own address and their token come in the pull request that serves them.

## Following claude-mem

At each new release that is wanted:

1. Read the contract diff first: `git diff <old> <new> -- src/ui/my-work/src/live/ src/ui/my-work/src/data.ts src/ui/my-work/src/data-rules.ts src/ui/my-work/test-fixtures/ src/services/worker/http/routes/WorkViewRoutes.ts src/services/work-view/` in the claude-mem checkout.
2. Run `fetch.sh` with the new tag, then the contract test: the new mapper must read oboete's answer. A mapper that changed what it reads is answered in `src/work_view.rs`, and its Rust test writes the snapshot again (`OBOETE_CONTRACT_WRITE=1`).
3. Run `build.sh`. When a patch no longer applies, port the patches onto the new tag: in a scratch git repository, commit the old tag's tree (commit `old`), then each patch as a commit of its own, in order, on a branch; commit the new tag's tree on a branch of its own from `old` (commit `new`), and replay the patches with `git rebase --onto new old <the patches' branch>`, resolving each conflict there. Write the patches again from the rebased commits (`git format-patch`, with their names kept), and run `build.sh` with them.
4. Check every package the bundle holds (decision 114): where it comes from, what it runs when installed, and the advisories against it. The diff of `assets/work-view/THIRD-PARTY-NOTICES.md` shows which packages are new or changed; for one whose locked source, version and integrity hash are those of the last update, the last check of its source and of what it installs stands, and its advisories are checked again. A text in `licences/` names the versions it was checked for, and the build stops at another: read that package's licence again and add the version.
5. Commit `upstream/`, `UPSTREAM`, the patches, the texts in `licences/` that changed and `assets/work-view/` together, with `src/work_view.rs` and the snapshot written again when step 2 changed them, after the Node test has run over the new snapshot. `NOTICE` names `UPSTREAM`, so it does not change.

## Advisories

Checked on 2026-11-18, for claude-mem v15.19.0. GitHub's Dependabot lists 13 advisories against the vendored package files: the work-view's lockfile and claude-mem's root `package.json`. Only one of the packages they name is in the bundle:

- `linkify-it` 3.0.3: two advisories of quadratic time on crafted text (GHSA-22p9-wv53-3rq4, GHSA-v245-v573-v5vm). `ansi-to-react` brings it in and calls it only for its `linkify` option. The pages render `<Ansi>` without that option (`components/ai-elements/terminal.tsx:215`), so its matcher never runs.

The other eleven name six packages, none of which is in the bundle (`THIRD-PARTY-NOTICES.md`):

- `hono` (five), `@hono/node-server` (two), `body-parser`, `minimatch` and `picomatch` (one each): the build installs them from the work-view's lockfile. They come with `shadcn`'s command-line tool (through `@modelcontextprotocol/sdk`, `express` and `ts-morph`) and with the build's tools (`picomatch`, through `vite` among others). The build runs with install scripts off, as a user who cannot read the owner's keys.
- `shell-quote` (one): only claude-mem's root `package.json` lists it, and the build never installs it.

## A run that was killed

`build.sh` and `fetch.sh` put things back however they end, but a run killed with SIGKILL, or a machine that stopped, runs no handler and leaves `ui/work-view/.lock` behind; so does a signal at the moment a run takes the lock, before its handler is set. With no build or fetch running, what the run's reserved folder holds says how far it got. No reserved folder: the run moved nothing, or it had finished and only its lock was left; the tree and the bundle in place are whole.

- `build.sh`, `assets/.work-view.*`:
  - `old`, and no `assets/work-view/`: the old bundle was moved aside and the new one never moved in. Move `old` back to `assets/work-view`.
  - Neither `old` nor `assets/work-view/`: there was no bundle before, and none moved in. Run `build.sh` again once the lock is gone.
  - Anything else: the bundle in place is whole.
- `fetch.sh`, `ui/work-view/.upstream.*`:
  - No `UPSTREAM`: the run stopped before the swap, or after it was done. The tree in place and its `UPSTREAM` line are a pair.
  - `UPSTREAM` and `new`: the new tree never moved in. With `old` too, the old tree was moved aside: move it back to `ui/work-view/upstream`.
  - `UPSTREAM` and `old`, and no `new`: the new tree moved in, without its line. Remove `ui/work-view/upstream/` if it is there, and move `old` back to `ui/work-view/upstream`.
  - `UPSTREAM` alone: a first fetch's new tree moved in, or a run putting things back had moved the old tree back, which may be a tree fetched before and not yet committed. Leave the tree in place, and read the reserved folder's `UPSTREAM`, which names the tag the run was fetching. Once the reserved folder and the lock are gone, run `fetch.sh` again with the tag the tree is to be: it makes the tree and its `UPSTREAM` line again, whatever is there.
- Or, to drop what is not committed, put back what is: move `ui/work-view/upstream/`, `ui/work-view/UPSTREAM` and `assets/work-view/` aside, run `git restore --source=HEAD --staged --worktree -- ui/work-view/upstream ui/work-view/UPSTREAM assets/work-view`, and remove what was moved aside. This discards a fetch or a build not yet committed. A plain checkout over the trees would leave the files a partial swap added.

Then remove the reserved folder, and only then `ui/work-view/.lock`.
