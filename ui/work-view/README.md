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
- `contract/` holds the contract between oboete's answer and the pages: `snapshot.json`, the answer of `GET /api/work-view` (`src/work_view.rs`) for a synthetic home, which the Rust test `work_view::tests::the_contract_snapshot_is_this_homes_answer` keeps current (it compares the two with times and keyed prints written as their kind; with `OBOETE_CONTRACT_WRITE=1` it writes this run's answer, whatever it is. In the first 15 minutes of a month, when the home's newest goals fall in the month before, it refuses to write and does not compare), and a Node test that runs claude-mem's mapper, unchanged, over it (`node --test 'ui/work-view/contract/*.test.mjs'`). A bump of claude-mem runs the new mapper over the same answer.
- `.github/workflows/work-view.yml` runs `build.sh` again in CI. It fails when the bundle it makes differs from the committed one by a byte.

The bundle is not served yet. Its data is `GET /api/work-view` (`src/work_view.rs`), which answers at the main page's address with its token; the pages' own address and their token come in the pull request that serves them.

## Following claude-mem

At each new release that is wanted:

1. Read the contract diff first: `git diff <old> <new> -- src/ui/my-work/src/live/ src/ui/my-work/src/data.ts src/ui/my-work/src/data-rules.ts src/ui/my-work/test-fixtures/ src/services/worker/http/routes/WorkViewRoutes.ts src/services/work-view/` in the claude-mem checkout.
2. Run `fetch.sh` with the new tag, then the contract test: the new mapper must read oboete's answer. A mapper that changed what it reads is answered in `src/work_view.rs`, and its Rust test writes the snapshot again (`OBOETE_CONTRACT_WRITE=1`).
3. Run `build.sh`. When a patch no longer applies, make it again against the new tree: put the old tree, with its patched copy on top, in a scratch git repository, and use `git apply --3way` or a rebase.
4. Read the diff of `assets/work-view/THIRD-PARTY-NOTICES.md` and check each package that is new or changed: where it comes from, what it runs when installed, and the advisories against it (decision 114).
5. Commit `upstream/`, `UPSTREAM`, the patches and `assets/work-view/` together. `NOTICE` names `UPSTREAM`, so it does not change.

## Advisories

Checked on 2026-11-18, for claude-mem v15.19.0. GitHub's Dependabot lists 13 advisories against the vendored lockfiles. Only one of the packages they name is in the bundle:

- `linkify-it` 3.0.3: two advisories of quadratic time on crafted text (GHSA-22p9-wv53-3rq4, GHSA-v245-v573-v5vm). `ansi-to-react` brings it in and calls it only for its `linkify` option. The pages render `<Ansi>` without that option (`components/ai-elements/terminal.tsx:215`), so its matcher never runs.

The other twelve name build tools and claude-mem's own server: `hono`, `@hono/node-server`, `body-parser`, `minimatch`, `picomatch`, and `shell-quote`. `shell-quote` comes from claude-mem's root `package.json`, which the build never installs. None of them is in the bundle (`THIRD-PARTY-NOTICES.md`). The build runs them with install scripts off, as a user who cannot read the owner's keys.
