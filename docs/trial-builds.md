# Trial builds

Trial builds are unfinished software, built from a checked commit of `main`.
The `trial-build` workflow runs by hand. Select `main`, enter the commit's
40 lowercase hex digits and a positive trial number. The commit must be on
`origin/main`. CI's run on it, as pushed to `main`, must have passed, and no
other check on it may be failing or still running (a skipped or neutral one
counts as passed). A pull request changing the workflow or its scripts runs the
same build and smoke tests without creating a release.

This workflow's own runs on the commit are left out of those checks: the run that
asks is still in progress, and one that failed before would bar the commit for good.
Runs started by hand go one at a time, so the second finds the first's draft and
stops; a third started while two are there takes the waiting one's place.

For package version `0.1.0` and trial number `1`, the version is `0.1.0-trial.1`
and the tag is `v0.1.0-trial.1`. The binary reports
`oboete 0.1.0-trial.1+<first 8 commit digits>`. The chosen commit must contain
the trial scripts and the build-version code.

Each target runs `cargo test --locked --features local-embed` before a clean
`cargo build --release --locked --features local-embed`, with the same environment.
There is no build cache. The three targets and floors are:

| Target | Floor |
| --- | --- |
| `x86_64-unknown-linux-gnu` | glibc 2.28, built in pinned manylinux_2_28; symbol versions checked and archive started in pinned AlmaLinux 8 |
| `aarch64-apple-darwin` | macOS 11, Apple silicon; deployment target 11.0 |
| `x86_64-pc-windows-msvc` | Windows x86_64; MSVC runtime linked statically |

An archive is `oboete-<VERSION>-<target>.tar.gz`, or `.zip` for Windows.
It contains one folder of the same name without the extension, holding only
`oboete` or `oboete.exe`, `LICENSE`, `NOTICE` and a generated `README.md` with
the commit, target, floor and setup instructions. The smoke test unpacks it,
checks these entries and the exact version line, then runs `doctor` with a new
empty home. It does not download embedding models or test the local model.

The workflow attaches the three archives and `SHA256SUMS` to a **draft
pre-release**, with build provenance for the archives. An existing tag or
release, including a draft, stops it; a run that failed while attaching the
archives can leave an incomplete draft, to delete before starting again. Review the run, notes and attachments,
then publish the draft by hand. The workflow never publishes it. Dry-run
artifacts are kept for seven days.

Download an archive and `SHA256SUMS` into one directory, then run:

```sh
sha256sum -c --ignore-missing SHA256SUMS
gh attestation verify <archive> --repo ojungo69/oboete
```

`--ignore-missing` checks the archives that are there (GNU coreutils). Where there
is no `sha256sum` (macOS), `shasum -a 256 <archive>` prints the archive's digest
to compare with its line in `SHA256SUMS`. After verification, put the binary where it will stay, for example
`~/.local/bin`, add that directory to `PATH`, then run `oboete setup <agent>`
and `oboete doctor`. The agents' hooks use the binary's path.

The toolchain is the stable Rust of the day, installed by rustup from its own
host, and the Linux build's tools are the image's repositories' current packages:
neither is pinned to a version. The build's log names the compiler it used.

Not verified on a real machine yet: a download by a browser on macOS
(quarantine) and on Windows (SmartScreen). Hosted-runner smoke tests also do not
prove operation on a real macOS 11 machine. This workflow's first run after
review must confirm the container toolchain, tests, glibc check, artifact names,
attestation and draft release.
