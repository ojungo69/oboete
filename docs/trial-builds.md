# Trial builds

Trial builds are unfinished software, built from a checked commit of `main`.
The `trial-build` workflow runs by hand. Select `main` and enter a positive trial
number: the run builds the commit `main` is at when it starts, and no input names
another. CI's run on that commit, as pushed to `main`, must have passed, and no
other check on it may be failing or still running (a skipped or neutral one
counts as passed): right after a merge, wait for CI. A pull request changing the
workflow or its scripts runs the same build and smoke tests, on its merge with
`main`, without creating a release.

This workflow's own runs on the commit are left out of those checks: the run that
asks is still in progress, and one that failed before would bar the commit for good.
Runs started by hand go one at a time, so the second finds the first's draft and
stops; a third started while two are there takes the waiting one's place.

For package version `0.1.0` and trial number `1`, the version is `0.1.0-trial.1`
and the tag is `v0.1.0-trial.1`. The binary reports
`oboete 0.1.0-trial.1+<first 8 commit digits>`.

The macOS and Windows builds run `cargo test --locked --features local-embed`
before a clean `cargo build --release --locked --features local-embed`, with the
same environment. The Linux build runs in a container, where 25 of the tests
failed in the workflow's first run: 2 that need a file the user cannot read (the
container's user is root), 18 that save a managed key file (refused with
`Protected`, as its check does on an overlay filesystem), and 5 whose synthetic
TLS server, run by the image's `python3` and `openssl`, did not answer (not
looked into further). It does not run the suite: on Linux the suite is CI's run
on the commit, which the workflow requires to have passed, and the smoke test
checks the version the build says.
There is no build cache. The three targets and floors are:

| Target | Floor |
| --- | --- |
| `x86_64-unknown-linux-gnu` | glibc 2.28, built in pinned manylinux_2_28; symbol versions checked and archive started in pinned AlmaLinux 8 |
| `aarch64-apple-darwin` | macOS 11, Apple silicon; deployment target 11.0 |
| `x86_64-pc-windows-msvc` | Windows 10, x86_64 (Rust's floor for the target); MSVC runtime linked statically |

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

To check a download, print its SHA-256 digest: it is the one on the archive's
line in `SHA256SUMS`. Then its build provenance:

```sh
sha256sum <archive>                        # Linux
shasum -a 256 <archive>                    # macOS
Get-FileHash -Algorithm SHA256 <archive>   # Windows PowerShell: the same digits, in capitals
gh attestation verify <archive> --repo ojungo69/oboete
```

The archive's own README names the command for its target. After verification, put the binary where it will stay, for example
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
