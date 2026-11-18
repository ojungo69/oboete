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
- `.github/workflows/work-view.yml` runs `build.sh` again in CI. It fails when the bundle it makes differs from the committed one by a byte.

Nothing here is served yet. The route, the pages' own address and their token come in the next pull request.

## Following claude-mem

At each new release that is wanted:

1. Read the contract diff first: `git diff <old> <new> -- src/ui/my-work/src/live/ src/ui/my-work/src/data.ts src/ui/my-work/src/data-rules.ts src/ui/my-work/test-fixtures/ src/services/worker/http/routes/WorkViewRoutes.ts src/services/work-view/` in the claude-mem checkout.
2. Run `fetch.sh` with the new tag.
3. Run `build.sh`. When a patch no longer applies, make it again against the new tree: put the old tree, with its patched copy on top, in a scratch git repository, and use `git apply --3way` or a rebase.
4. Read the diff of `assets/work-view/THIRD-PARTY-NOTICES.md` and check each package that is new or changed: where it comes from, what it runs when installed, and the advisories against it (decision 114).
5. Commit `upstream/`, `UPSTREAM`, the patches and `assets/work-view/` together. `NOTICE` names `UPSTREAM`, so it does not change.
