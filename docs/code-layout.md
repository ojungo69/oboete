# Where the code is (2026-11-14)

How `src/` is laid out, and why. Decided by the one who maintains it: an agent that comes to the
code with nothing in mind each session and reads before it changes. So the measure of the layout
is what one read costs: a file is opened to change one thing, and what is not that thing is
cost.

## The rules

1. **A module's unit tests are a file of their own.** The tests of `src/a.rs` are
   `src/a/tests.rs`, declared in `a.rs` as `#[cfg(test)] mod tests;` at the place the inline
   module stood. A test's path does not change by it (`a::tests::name`), so nothing that names a
   test changes. Other modules that only test builds compile have their own files the same way:
   `fixture.rs`, `stub.rs`, `fixture_scan.rs`, `input_tests.rs`, and `src/crash.rs`. The crate
   root's tests are `src/tests.rs`. The rule is for files of code: a test's own file holds
   modules as it likes (`keyfile/tests.rs` has one for Linux).
2. **`sonar-project.properties` declares those files as tests** (`sonar.test.inclusions`), by
   their names. A test-only module with a new name is added there.
3. **A move is a pull request of its own.** No fix and no rewording rides on it; its proof is
   mechanical (below), since nobody can read a diff of that size.
4. **The pin of the matching code** (`the_matching_code_is_the_one_its_number_names`) hashes
   `src/redact.rs` up to the line that declares its tests. If matching code ever moves to
   another file, the pin is widened to that file in the same change.

## Why tests moved out (R1)

Measured at 474a620f: 237,413 lines of Rust in 105 files under `src/`, 129,283 of them (54%)
inline test modules, 100 modules in 96 files. `curate.rs` was 15,303 lines for 5,731 of code;
`forget.rs` 11,042 for 2,479; `hook.rs` 13,212 for 4,161. To read a module's code was to page
past its tests or to trust a line number; to change a test was to open the code. After the move
the largest file of code is `raw.rs` (9,793 lines) and the largest test file is
`curate/tests.rs` (9,548).

What the move did not change: any line of code outside test modules (each file's text before
and after its test module is the same bytes), the list of tests (3,019 names, 3,020 with
`local-embed`, the same before the move and after its `cargo fmt`; the test that holds this
layout came after as one more), the pin's value, and the release binary: the same sha256 before
and after, with and without `local-embed`. One file had code after its tests,
`src/setup/agents.rs`: that code moved up 109 lines, and the binary of that step has the same
size and 55 other bytes, the line numbers its panic messages carry.

How a move of this kind is proved, for the next one:

- The script that moves the bytes checks, for each file, that the original is exactly
  `before + opener + body + close + after`, and the new parent `before + declaration + after`.
  It re-indents nothing: `cargo fmt` does, in a commit of its own that is that command's output
  and nothing else (run twice: the first pass leaves an empty first line in each file it takes
  the indentation from, the second takes it out).
- `cargo test -- --list` gives the same names before and after, with and without `local-embed`.
- The hand edits are listed one by one in the pull request: the places that read the layout
  (an `include_str!` path relative to the file, a test that reads a source file).

## What is left to split, and the measure for it

Without tests, 14 files hold more than 2,000 lines of code: `raw.rs` 9,793, `curate.rs` 5,731,
`provider.rs` 4,664, `hook.rs` 4,161, `setup.rs` 4,041, `view.rs` 3,727, `search/b.rs` 3,640,
`my_work.rs` 2,835, `backup.rs` 2,832, `worker.rs` 2,569, `forget.rs` 2,479, `embed_phase.rs`
2,269, `redact.rs` 2,064, `settings.rs` 2,021.

A file is split when a part of it can be named for what it does, is changed for other reasons
than the rest, and can be read without the rest (its items reach into the rest through a few
names, not through its private state). Size alone is not the reason: a file of one thing that
is long stays one file. A split is decided file by file, after the findings of the audit
against that file are fixed (the register of 2026-11-11 names places by their text, and a fix
is easier to review on the layout it was found in). A split keeps every path that other
modules use (`pub(crate) use` from the parent), so that it is a move and its proof is the same
as above.

The twelve `my_work_*.rs` modules are one family beside `my_work.rs`; they become
`my_work/<name>.rs` in one move when no other change is open against them.
