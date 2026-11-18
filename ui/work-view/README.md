# claude-mem's work-view

Decision 114 (2026-11-17) serves claude-mem's work-view on oboete's engine: My Work, focus areas, Suggested, Past focus and My Rules ([page.md](../../docs/page.md), "claude-mem's My Work pages"). The spike that came first is [spike/work-view.md](../../docs/spike/work-view.md).

- `upstream/` is the work-view as claude-mem published it, unchanged. It holds the sources, the files outside `src/ui/my-work` they import, and claude-mem's root `package.json`, which sets `"type": "module"`. `LICENSE` and `NOTICE` are claude-mem's. claude-mem's `src/` layout is kept, because the sources' relative imports climb to it.
- `UPSTREAM` names the tag and the commit `upstream/` came from.
- `fetch.sh` makes `upstream/` again from a claude-mem checkout at a tag: `ui/work-view/fetch.sh <checkout> <tag>`.

Nothing here is built or served yet. The patches, the build and the bundle the viewer serves come in the next pull request, kept apart from `upstream/`, so that `upstream/` stays exactly what was published.

## Following claude-mem

At each new release that is wanted:

1. Read the contract diff first: `git diff <old> <new> -- src/ui/my-work/src/live/ src/ui/my-work/src/data.ts src/ui/my-work/src/data-rules.ts src/ui/my-work/test-fixtures/ src/services/worker/http/routes/WorkViewRoutes.ts src/services/work-view/` in the claude-mem checkout.
2. Run `fetch.sh` with the new tag.
3. Check every package the bundle holds: where it comes from, what it runs when installed, and the advisories against it (decision 114).
